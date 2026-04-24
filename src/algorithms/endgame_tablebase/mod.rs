use super::{AlgorithmId, MoveResult};
use crate::board::Board;
use crate::tables::execute_move;
use std::cell::RefCell;
use std::sync::OnceLock;

const TABLE_BYTES: &[u8] = include_bytes!("../../../data/endgame_tablebase.bin");
const TABLE_MAGIC: &[u8; 8] = b"P8ETB001";
const DIAGONAL_CANONICAL_FLAG: u32 = 1;
const BIT_OFFSETS: [u32; 10] = [12, 28, 32, 36, 40, 44, 48, 52, 56, 60];
const POWERS_OF_9: [u32; 10] = [
    1, 9, 81, 729, 6561, 59049, 531441, 4782969, 43046721, 387420489,
];
const MAP_MOVE: [[usize; 5]; 8] = [
    [0, 1, 2, 3, 4],
    [0, 3, 4, 2, 1],
    [0, 2, 1, 4, 3],
    [0, 4, 3, 1, 2],
    [0, 2, 1, 3, 4],
    [0, 1, 2, 4, 3],
    [0, 3, 4, 1, 2],
    [0, 4, 3, 2, 1],
];
const TILE_WEIGHT_MAP: [i32; 16] = [
    0, 2, 4, 8, 16, 32, 64, 128, 248, 388, 488, 518, 519, 519, 519, 520,
];
const MAX_CACHE_BUCKETS: usize = 1 << 19;
const CACHE_WAYS: usize = 8;
const SCORE_CRITICAL: i32 = -30_000;
const SCORE_HOPELESS: i32 = -100_000;
const FALLBACK_DEPTH: i32 = 5;

static TABLEBASE: OnceLock<Option<Tablebase>> = OnceLock::new();
static EVAL_TABLES: OnceLock<Box<[ScorePair; 65536]>> = OnceLock::new();

thread_local! {
    static CORE_AI: RefCell<CoreAiLogic> = RefCell::new(CoreAiLogic::new());
}

#[derive(Clone, Copy)]
struct Layer {
    buckets: u32,
    len: u32,
    seed_offset: u32,
    sig_offset: u32,
    rate_offset: u32,
}

struct Table {
    table_type: u32,
    initial_sum: i32,
    lower: f32,
    upper: f32,
    flags: u32,
    layers: Vec<Layer>,
}

struct Tablebase {
    tables: Vec<Table>,
    blob: &'static [u8],
}

#[derive(Clone, Copy)]
struct ProbeResult {
    move_id: usize,
    is_evil: bool,
    table_type: u32,
    rates: [f32; 4],
    threshold: u8,
    queries: u64,
}

#[derive(Clone, Copy)]
struct ScorePair {
    d1: i16,
    d2: i16,
}

#[derive(Clone, Copy)]
struct EvalOverride {
    index: usize,
    index2: usize,
    score: i16,
}

#[derive(Clone, Copy)]
struct SearchOutcome {
    best_operation: usize,
    depth: i32,
    scores: [i32; 4],
    nodes: u64,
    cache_hits: u32,
}

struct CoreAiLogic {
    ai_player: AiPlayer,
    last_depth: i32,
    last_sum: i32,
    last_prune: i32,
    time_ratio: f64,
}

impl CoreAiLogic {
    fn new() -> Self {
        Self {
            ai_player: AiPlayer::new(),
            last_depth: 4,
            last_sum: 0,
            last_prune: 0,
            time_ratio: 4.0,
        }
    }

    fn calculate_step(&mut self, tablebase: &Tablebase, upstream_board: Board) -> MoveResult {
        self.ai_player.reset_board(upstream_board);
        let counts = tile_counts(upstream_board);
        let board_sum = board_sum_from_counts(counts);
        let big_nums = sum_counts(&counts, 8, 16) as i32;
        let probe_result = probe(tablebase, upstream_board, counts, board_sum);
        let mut total_nodes = probe_result.queries;
        let mut cache_hits = 0;

        if probe_result.move_id != 0
            && legal_internal_move(upstream_board, probe_result.move_id)
            && self.validate_egtb_move(
                tablebase,
                upstream_board,
                probe_result.move_id,
                probe_result.table_type,
                probe_result.rates,
                board_sum,
                probe_result.threshold,
            )
        {
            return MoveResult {
                algorithm: AlgorithmId::EndgameTablebase,
                move_id: internal_to_api_move(probe_result.move_id),
                depth: probe_result.threshold as i32,
                nodes: total_nodes + self.ai_player.node,
                cache_hits: probe_result.table_type,
            };
        }

        let is_not_merging = max_counts(&counts, 8, 15) == 1
            && !(counts[7] > 1 && counts[8] == 1)
            && !(counts[6] > 1 && counts[7] == 1 && counts[8] == 1 && board_sum % 1024 < 96);
        let is_mess = is_not_merging && is_mess(upstream_board, board_sum);
        let is_5tiler = is_5tiler(board_sum, &counts);
        self.ai_player.do_check = if is_mess && (3..=6).contains(&big_nums) {
            big_nums
        } else {
            0
        };

        let mut prune = 0;
        if is_not_merging {
            let cond1 = !(40 < board_sum % 512 && board_sum % 512 < 500)
                && max_counts(&counts, 7, 9) > 1
                && big_nums > 2;
            let cond2 = !(32 < board_sum % 256 && board_sum % 256 < 250)
                && max_counts(&counts, 6, 8) > 1
                && big_nums > 4;
            let cond3 = !(24 < board_sum % 128 && board_sum % 128 < 126)
                && max_counts(&counts, 5, 7) > 1
                && big_nums > 4;
            if !(cond1 || cond2 || cond3 || is_mess) {
                prune = 1;
            }
        }
        self.ai_player.prune = prune;

        let is_sparse_endgame = max_counts(&counts, 6, 16) == 1 && sum_counts(&counts, 6, 16) >= 9;
        if (is_mess || probe_result.is_evil || tiles_all_set(&counts) || is_sparse_endgame)
            && !is_5tiler
        {
            self.ai_player.prune = 0;
        }
        if danbianhuichuan_patch(upstream_board, board_sum) {
            self.ai_player.prune = 1;
        }

        let empty_slots = i32::from(counts[0]);
        let is_not_merging = is_not_merging;
        let outcome = if is_mess || is_5tiler {
            let big_nums2 = sum_counts(&counts, 9, 16) as f64;
            self.perform_iterative_search(5, 24, 1.2 * big_nums2.powf(0.25))
        } else if empty_slots > 4 && big_nums < 2 && is_not_merging {
            self.perform_iterative_search(3, 3, 0.1)
        } else if ((big_nums <= 3
            && 32 < board_sum % 256
            && board_sum % 256 < 248
            && is_not_merging)
            || big_nums < 3)
            && !(board_sum % 256 < 72 && counts[6] > 0)
        {
            let depth = if counts[7] == 0 { 4 } else { 5 };
            self.perform_iterative_search(depth, depth, 0.1)
        } else {
            let (mut initial_depth, mut max_depth, time_limit) = if 65380 < board_sum
                && board_sum <= 65500
            {
                (((65540 - board_sum) / 2).min(33), 60, 0.8)
            } else if 65260 < board_sum && board_sum <= 65380 {
                (20, 60, 1.0)
            } else if counts[7] > 1 || (board_sum % 512 < 20 && sum_counts(&counts, 8, 16) > 4) {
                (4, 32, 0.32 * (big_nums as f64).powf(0.4))
            } else if is_not_merging && sum_counts(&counts, 7, 16) > 5 {
                (4, 48, 0.32 * (big_nums as f64).powf(0.25))
            } else {
                (4, 24, 0.16 * (big_nums as f64).powf(0.25))
            };
            initial_depth += self.ai_player.prune;
            if !is_mess && sum_counts(&counts, 9, 16) <= 3 {
                max_depth = 10;
            }
            if self.ai_player.prune != 0 && (board_sum - self.last_sum).abs() < 6 {
                let min_initial =
                    (self.last_depth - 1).min(round_small_i32(f64::from(self.last_depth) * 0.9));
                initial_depth = initial_depth.max(min_initial);
            }
            self.perform_iterative_search(initial_depth, max_depth, time_limit)
        };

        self.last_sum = board_sum;
        self.last_depth = outcome.depth;
        self.last_prune = self.ai_player.prune;
        total_nodes += outcome.nodes;
        cache_hits += outcome.cache_hits;

        let best_operation = legalize_internal_move(upstream_board, outcome.best_operation);
        MoveResult {
            algorithm: AlgorithmId::EndgameTablebase,
            move_id: internal_to_api_move(best_operation),
            depth: outcome.depth,
            nodes: total_nodes,
            cache_hits,
        }
    }

    fn perform_iterative_search(
        &mut self,
        initial_depth: i32,
        max_depth: i32,
        time_limit: f64,
    ) -> SearchOutcome {
        let mut best_op = 0;
        let mut final_depth = 0;
        let mut valid_scores = [-self.ai_player.dead_score; 4];
        let mut total_nodes = 0;
        let mut total_hits = 0;
        let mut local_limit = time_limit;
        let mut depth = initial_depth.max(1);

        while depth <= max_depth {
            self.ai_player
                .start_search(depth, node_budget(depth, local_limit));
            total_nodes += self.ai_player.node;
            total_hits += self.ai_player.cache_hits;
            if self.ai_player.stop_search {
                break;
            }
            best_op = self.ai_player.best_operation;
            final_depth = depth;
            valid_scores = self.ai_player.top_scores;
            let best_score = if (1..=4).contains(&best_op) {
                valid_scores[best_op - 1]
            } else {
                0
            };
            if best_score < SCORE_CRITICAL {
                local_limit = time_limit + 1.0 + 0.1 * f64::from(depth - initial_depth);
            }
            if valid_scores
                .into_iter()
                .max()
                .unwrap_or(-self.ai_player.dead_score)
                < SCORE_HOPELESS
            {
                break;
            }
            depth += 1;
            if depth > initial_depth + 2 && time_limit <= 0.11 {
                break;
            }
            self.time_ratio = (self.time_ratio * 0.75 + 1.2 * 0.25).clamp(1.2, 12.0);
        }

        if best_op == 0 {
            self.ai_player.prune = 0;
            self.ai_player.clear_cache();
            self.ai_player
                .start_search(FALLBACK_DEPTH, node_budget(FALLBACK_DEPTH, 2.0));
            total_nodes += self.ai_player.node;
            total_hits += self.ai_player.cache_hits;
            best_op = self.ai_player.best_operation;
            final_depth = FALLBACK_DEPTH;
            valid_scores = self.ai_player.top_scores;
        }

        SearchOutcome {
            best_operation: best_op,
            depth: final_depth,
            scores: valid_scores,
            nodes: total_nodes,
            cache_hits: total_hits,
        }
    }

    fn validate_egtb_move(
        &mut self,
        tablebase: &Tablebase,
        board: Board,
        move_id: usize,
        table_type: u32,
        win_rates: [f32; 4],
        board_sum: i32,
        threshold: u8,
    ) -> bool {
        let max_win = max_rate(win_rates);
        if table_type == 512 && board_sum % 512 > 506 && 0.91109 < max_win && max_win < 0.91111 {
            return true;
        }

        self.ai_player.prune = i32::from(48 < board_sum % 256 && board_sum % 256 < 234);
        self.ai_player.do_check = 0;
        let depth = if self.ai_player.prune != 0 { 8 } else { 6 };
        self.ai_player.start_search(depth, node_budget(depth, 1.0));
        let scores = self.ai_player.top_scores;
        let win_best = argmax(win_rates);
        let score_best = argmax_i32(scores);
        let max_score = scores[score_best];

        if table_type == 1256 && max_score - scores[win_best] < 50 {
            return true;
        }

        let mut need_further_check = false;
        if win_rates[score_best] == 0.0 && max_score - scores[win_best] > 5 {
            if !((table_type == 256 && max_win > 0.993)
                || (table_type == 512 && board_sum % 512 < 64 && max_win > 0.84))
            {
                need_further_check = true;
            }
        }

        if !need_further_check {
            let target_score = scores[move_id - 1];
            let sorted = sorted_desc(scores);
            if (target_score >= sorted[0] - 16 && sorted[2] > 2400)
                || (target_score >= sorted[0] - 24 && sorted[2] > 2800)
                || target_score >= sorted[0] - 8
            {
                return true;
            }
        }

        let max_d = if table_type == 1256 { 48 } else { 10 };
        let min_d = if table_type == 1256 { 24 } else { 6 };
        let time_limit = if table_type == 1256 { 0.64 } else { 0.32 };
        let outcome = self.perform_iterative_search(min_d, max_d, time_limit);
        if outcome.depth == 0 {
            return false;
        }
        let scores = outcome.scores;
        let max_score = scores[argmax_i32(scores)];

        if table_type == 1256 && max_score - scores[win_best] < 100 {
            return true;
        }

        let target_score = scores[move_id - 1];
        let sorted = sorted_desc(scores);
        if target_score < -self.ai_player.dead_score / 2 && sorted[0] > 0 {
            return false;
        }
        if (target_score >= sorted[0] - 24 && sorted[2] > 2400)
            || (target_score >= sorted[0] - 36 && sorted[2] > 2800)
            || target_score >= sorted[0] - 12
        {
            return true;
        }

        if !(threshold == 8 && table_type == 512 && board_sum % 256 < 96) {
            let after_best = execute_internal_move(board, outcome.best_operation);
            let after_table = execute_internal_move(board, move_id);
            let win_rate1 = probe_after_move_runtime(
                tablebase,
                after_best,
                threshold,
                &[table_type],
                board_sum,
            );
            let win_rate2 = probe_after_move_runtime(
                tablebase,
                after_table,
                threshold,
                &[table_type],
                board_sum,
            );
            if win_rate2.max(win_rate1) < 0.2 {
                return false;
            }
            if win_rate2 > win_rate1
                && ((win_rate1 > 0.0 || target_score > 2000)
                    || (win_rate1 == 0.0
                        && target_score < -3000
                        && 60 < board_sum % 256
                        && board_sum % 256 < 200))
            {
                return true;
            }
        }
        false
    }
}

struct AiPlayer {
    stop_search: bool,
    max_d: i32,
    max_layer: i32,
    best_operation: usize,
    board: Board,
    board_sum: i32,
    node: u64,
    cache_hits: u32,
    spawn_rate4: f64,
    spawn_weight_4: i32,
    spawn_weight_2: i32,
    do_check: i32,
    masked_count: i32,
    fixed_mask: Board,
    prune: i32,
    initial_eval_score: i32,
    threshold: i32,
    dead_score: i32,
    top_scores: [i32; 4],
    eval_overrides: [EvalOverride; 8],
    cache: SearchCache,
    node_budget: u64,
}

impl AiPlayer {
    fn new() -> Self {
        Self {
            stop_search: false,
            max_d: 3,
            max_layer: 5,
            best_operation: 0,
            board: 0,
            board_sum: 0,
            node: 0,
            cache_hits: 0,
            spawn_rate4: 0.1,
            spawn_weight_4: 6553,
            spawn_weight_2: 58983,
            do_check: 0,
            masked_count: 0,
            fixed_mask: 0,
            prune: 0,
            initial_eval_score: 0,
            threshold: 4000,
            dead_score: 131072,
            top_scores: [-131072; 4],
            eval_overrides: default_eval_overrides(),
            cache: SearchCache::new(),
            node_budget: u64::MAX,
        }
    }

    fn reset_board(&mut self, board: Board) {
        self.best_operation = 0;
        self.board = board;
        self.node = 0;
        self.cache_hits = 0;
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn start_search(&mut self, depth: i32, node_budget: u64) {
        self.stop_search = false;
        self.best_operation = 0;
        self.max_d = depth;
        self.max_layer = get_max_layer(self.spawn_rate4, depth);
        self.node_budget = node_budget.max(1024);
        let buckets = if depth < 3 {
            4096
        } else if depth < 4 {
            16384
        } else if depth < 5 {
            65536
        } else if depth < 7 {
            262144
        } else {
            MAX_CACHE_BUCKETS
        };
        self.cache.reset(buckets);
        self.initial_eval_score = self.evaluate(self.board);
        self.node = 0;
        self.cache_hits = 0;
        self.top_scores = [-self.dead_score; 4];
        let masked_board = self.apply_dynamic_mask();
        let _ = self.search_ai_player(masked_board, self.max_d, 0);
        if self.fixed_mask != 0 && self.best_operation > 0 {
            let best_score = self.top_scores[self.best_operation - 1];
            if best_score < (-2000).min(-(self.dead_score >> 2))
                || (self.masked_count < 5
                    && best_score < 480 * self.masked_count
                    && self.board_sum % 512 < 160)
            {
                self.fixed_mask = 0;
                self.masked_count = 0;
                self.threshold = self.threshold.max(6000);
                self.cache.clear();
                let _ = self.search_ai_player(self.board, self.max_d, 0);
            }
        }
    }

    fn score_pair(&self, row: usize) -> ScorePair {
        let mut pair = eval_tables()[row];
        for override_entry in self.eval_overrides {
            if override_entry.index == row {
                pair.d1 = override_entry.score;
            }
            if override_entry.index2 == row {
                pair.d2 = override_entry.score;
            }
        }
        pair
    }

    fn evaluate(&self, board: Board) -> i32 {
        let transposed = reverse_ul(board);
        let mut sum_x1 = 0;
        let mut sum_x2 = 0;
        let mut sum_y1 = 0;
        let mut sum_y2 = 0;
        for i in 0..4 {
            let l1 = ((board >> (16 * i)) & 0xffff) as usize;
            let l2 = ((transposed >> (16 * i)) & 0xffff) as usize;
            let px = self.score_pair(l1);
            sum_x1 += i32::from(px.d1);
            sum_x2 += i32::from(px.d2);
            let py = self.score_pair(l2);
            sum_y1 += i32::from(py.d1);
            sum_y2 += i32::from(py.d2);
        }
        let mut result = sum_x1.max(sum_x2) + sum_y1.max(sum_y2);
        if self.do_check != 0 {
            result -= self.check_corner(board);
        }
        result
    }

    fn search0(&mut self, board: Board) -> i32 {
        self.node = self.node.saturating_add(4);
        let mut best = -self.dead_score;
        for internal_move in 1..=4 {
            let moved = execute_internal_move(board, internal_move);
            if moved != board {
                let score = move_score(board, internal_move);
                best = best.max(self.evaluate(moved) + process_score(score));
            }
        }
        best
    }

    fn search_branch(&mut self, board: Board, depth: i32, sum_increment: i32) -> (i32, u64) {
        let mut out_nodes = 1;
        self.node = self.node.saturating_add(1);
        if self.node >= self.node_budget {
            self.stop_search = true;
            return (0, out_nodes);
        }
        if depth < self.max_d - 2 {
            let current_eval = self.evaluate(board);
            if sum_increment > self.max_layer {
                return (current_eval, out_nodes);
            }
            if (board & self.fixed_mask) != self.fixed_mask {
                return (-self.dead_score, out_nodes);
            }
            if current_eval < self.initial_eval_score - self.threshold {
                return (-self.dead_score, out_nodes);
            }
        }

        let mut empty_mask = empty_cell_mask(board);
        let empty_slots = empty_mask.count_ones() as i32;
        if empty_slots == 0 {
            let (score, child_nodes) = self.search_ai_player(board, depth - 1, sum_increment);
            return (score, out_nodes + child_nodes);
        }
        let mut effective_depth = depth;
        if empty_slots > 5 && self.masked_count < 4 {
            effective_depth = effective_depth.min(3);
        } else if empty_slots > 4 && self.masked_count < 4 {
            effective_depth = effective_depth.min(4);
        }

        let cache_idx = self.cache.hash(board);
        if let Some(cached) = self.cache.lookup(cache_idx, board, effective_depth) {
            self.cache_hits += 1;
            return (cached, out_nodes + 1);
        }

        let mut local = 0i64;
        while empty_mask != 0 {
            let bit_pos = empty_mask.trailing_zeros();
            let t4 = board | (2u64 << bit_pos);
            let (score4, nodes4) =
                self.search_ai_player(t4, effective_depth - 1, sum_increment + 2);
            let t2 = board | (1u64 << bit_pos);
            let (score2, nodes2) =
                self.search_ai_player(t2, effective_depth - 1, sum_increment + 1);
            out_nodes += nodes4 + nodes2;
            local += ((i64::from(score2) * i64::from(self.spawn_weight_2))
                + (i64::from(score4) * i64::from(self.spawn_weight_4)))
                >> 16;
            empty_mask &= empty_mask - 1;
            if self.stop_search {
                break;
            }
        }
        let score = (local / i64::from(empty_slots)) as i32;
        self.cache.update(
            cache_idx,
            board,
            effective_depth,
            score,
            self.dead_score,
            out_nodes,
        );
        (score, out_nodes)
    }

    fn search_ai_player(&mut self, board: Board, depth: i32, sum_increment: i32) -> (i32, u64) {
        let mut out_nodes = 1;
        if self.stop_search {
            return (0, out_nodes);
        }
        if depth <= 0 {
            return (self.search0(board), out_nodes);
        }
        let mut best = -self.dead_score;
        for internal_move in 1..=4 {
            let moved = execute_internal_move(board, internal_move);
            if moved == board {
                continue;
            }
            let score = move_score(board, internal_move);
            let mut current_depth = depth;
            if depth < self.max_d - 2 && score > 250 && score < 2000 {
                let min_depth = (score as u32).leading_zeros() as i32 - 16;
                current_depth = min_depth.min(current_depth.max(2));
            }
            let (branch_score, branch_nodes) =
                self.search_branch(moved, current_depth, sum_increment);
            out_nodes += branch_nodes;
            let mut temp = branch_score + process_score(score);
            if depth >= self.max_d - 2 {
                temp += 1;
            }
            if temp > best {
                best = temp;
                if depth == self.max_d {
                    self.best_operation = internal_move;
                }
            }
            if depth == self.max_d {
                self.top_scores[internal_move - 1] = temp;
            }
            if self.stop_search {
                break;
            }
        }
        (best, out_nodes)
    }

    fn apply_dynamic_mask(&mut self) -> Board {
        let counts = tile_counts(self.board);
        self.board_sum = board_sum_from_counts(counts);
        self.fixed_mask = 0;
        let small_tiles_sum: i32 = (1..9)
            .map(|rank| i32::from(counts[rank]) * (1 << rank))
            .sum();
        let count_gt_128 = sum_counts(&counts, 8, 16) as i32;
        let count_gt_256 = sum_counts(&counts, 9, 16) as i32;
        let distinct_gt_256 = !(9..15).any(|rank| counts[rank] > 1);
        let large_tiles = large_tile_count(8, &counts);

        self.threshold = if large_tiles > 4 {
            if self.prune == 0 {
                5600
            } else {
                2400
            }
        } else if self.prune == 0 {
            8400
        } else {
            3200
        };

        self.dead_score = if count_gt_128 <= 4 {
            262144
        } else if self.board_sum % 1024 < 8 || self.board_sum % 1024 > 1008 {
            131072
        } else if count_gt_128 == 5 {
            131072
        } else if count_gt_256 > 5 && distinct_gt_256 {
            if count_gt_128 == 8 && (counts[7] == 1 || counts[6] >= 1) {
                4096
            } else if count_gt_256 >= 6 && 960 < self.board_sum % 1024 {
                32768
            } else {
                48000
            }
        } else {
            65536
        };

        if !distinct_gt_256 {
            return self.board;
        }

        let rem = self.board_sum % 1024;
        let cond_a_part1 =
            (rem >= 48 || (rem > 12 && small_tiles_sum == rem)) && rem <= 512 && counts[9] == 0;
        let cond_a_part2 = rem >= 512 || rem < 6;
        let current_board = if cond_a_part1 || cond_a_part2 {
            if rem > 1000 {
                mask_large_tiles(self.board, 11, 0xf)
            } else {
                mask_large_tiles(self.board, 9, 0xf)
            }
        } else {
            mask_large_tiles(self.board, 12, 0xf)
        };

        self.masked_count =
            ((current_board & (current_board >> 1) & (current_board >> 2) & 0x1111_1111_1111_1111)
                .count_ones()) as i32;
        let mut min_masked_tile = 0u8;
        if self.masked_count > 0 {
            let mut accumulated = 0;
            for rank in (0..16).rev() {
                accumulated += i32::from(counts[rank]);
                if accumulated >= self.masked_count {
                    min_masked_tile = rank as u8;
                    break;
                }
            }
        }

        let count_gt_512 = sum_counts(&counts, 10, 16) as i32;
        let flag1 =
            count_gt_512 == 5 && (counts[9] == 1 || counts[8] > 0) && self.board_sum % 512 < 72;
        let flag2 = ((count_gt_512 == 6 || (count_gt_512 == 5 && counts[9] == 1))
            && self.board_sum % 1024 < 72)
            || (count_gt_512 == 5
                && counts[9] == 0
                && (counts[8] == 1 || counts[7] > 0)
                && self.board_sum % 256 < 72);
        let flag3 = count_gt_512 >= 4
            && self.masked_count >= 5
            && self.board_sum % 256 > 132
            && self.board_sum % 256 < 234;
        let flag4 =
            count_gt_512 == 4 && counts[9] == 1 && counts[10] == 1 && self.board_sum % 256 < 60;
        self.update_specific_scores(current_board, flag1, flag2, flag3, flag4);

        if self.masked_count >= 5 && counts[0] >= 3 {
            self.fixed_mask = get_22_mask(current_board) | get_21_mask(current_board);
        } else if (min_masked_tile == 12 && self.masked_count == 4)
            || (min_masked_tile == 11 && self.masked_count == 5)
            || (min_masked_tile == 10 && self.masked_count == 6)
        {
            self.fixed_mask = get_23_mask(current_board) | get_22_mask(current_board);
        } else if self.masked_count >= 5
            && (large_tiles > 5
                || (self.board_sum % 256 > 48 && self.board_sum % 256 < 224)
                || is_5tiler(self.board_sum, &counts))
        {
            self.fixed_mask = get_23_mask(current_board) | get_22_mask(current_board);
        } else if self.do_check != 0
            || (self.prune == 0 && (large_tiles > 2 || large_tiles == 0))
            || ((self.board_sum % 256 > 246 || self.board_sum % 256 < 24)
                && ((large_tiles < 5 && large_tiles > 2) || large_tiles == 0))
            || counts[7] > 1
            || (counts[6] > 1 && counts[7] == 1)
        {
            self.fixed_mask = 0;
        } else if self.masked_count >= 4 {
            self.fixed_mask = get_22_mask(current_board) | get_21_mask(current_board);
        } else if self.masked_count == 3 && count_gt_512 == 3 && self.board_sum % 1024 > 24 {
            self.fixed_mask = get_21_mask(current_board) | get_20_mask(current_board);
        } else if self.masked_count >= 2 && count_gt_512 == 2 && self.board_sum % 512 > 16 {
            self.fixed_mask = get_20_mask(current_board);
        }

        current_board
    }

    fn update_specific_scores(
        &mut self,
        board: Board,
        flag1: bool,
        flag2: bool,
        flag3: bool,
        flag4: bool,
    ) {
        let apply_bonus = get_22_mask(board) > 0;
        let mut overrides = default_eval_overrides();

        for target in &mut overrides[..3] {
            if apply_bonus && flag3 {
                target.score += target_bonus(target.index);
            }
        }

        for target in &mut overrides[3..] {
            if apply_bonus {
                let (bonus_f1, bonus_f2) = dynamic_target_bonuses(target.index);
                if flag1 {
                    target.score += bonus_f1;
                } else if flag2 {
                    target.score += bonus_f2;
                } else if flag4 {
                    target.score += bonus_f1 >> 1;
                }
            }
        }

        self.eval_overrides = overrides;
    }

    fn check_corner(&self, board: Board) -> i32 {
        check_corner_penalty(board, self.masked_count)
    }
}

struct SearchCache {
    entries: Vec<u64>,
    buckets: usize,
    mask: u64,
}

impl SearchCache {
    fn new() -> Self {
        Self {
            entries: vec![0; MAX_CACHE_BUCKETS * CACHE_WAYS],
            buckets: MAX_CACHE_BUCKETS,
            mask: (MAX_CACHE_BUCKETS - 1) as u64,
        }
    }

    fn reset(&mut self, buckets: usize) {
        self.buckets = buckets.min(MAX_CACHE_BUCKETS).max(1).next_power_of_two();
        self.mask = (self.buckets - 1) as u64;
        self.entries[..self.buckets * CACHE_WAYS].fill(0);
    }

    fn clear(&mut self) {
        self.entries[..self.buckets * CACHE_WAYS].fill(0);
    }

    fn hash(&self, board: Board) -> usize {
        (((((board ^ (board >> 27)).wrapping_mul(0x1a85_ec53)).wrapping_add(board)) >> 23)
            & self.mask) as usize
    }

    fn signature(board: Board) -> u32 {
        (((board ^ (board >> 31))
            .wrapping_mul(0x1a7d_af1b)
            .wrapping_add(board))
            >> 21) as u32
    }

    fn lookup(&self, bucket_idx: usize, board: Board, depth: i32) -> Option<i32> {
        let sig = Self::signature(board);
        let start = bucket_idx * CACHE_WAYS;
        for &entry in &self.entries[start..start + CACHE_WAYS] {
            if entry == 0 || (entry >> 32) as u32 != sig {
                continue;
            }
            let cached_depth = ((entry >> 6) & 0x3f) as i32;
            if cached_depth >= depth {
                return Some(((entry >> 12) & 0xfffff) as i32 - 524288);
            }
        }
        None
    }

    fn update(
        &mut self,
        bucket_idx: usize,
        board: Board,
        mut depth: i32,
        score: i32,
        dead_score: i32,
        subtree_nodes: u64,
    ) {
        if score <= -dead_score + 32 {
            depth = 63;
        }
        let sig = Self::signature(board);
        let pack_depth = depth.clamp(0, 63) as u64;
        let pack_effort = 63 - subtree_nodes.max(1).leading_zeros() as u64;
        let pack_score = ((score + 524288) as u64) & 0xfffff;
        let new_entry =
            (u64::from(sig) << 32) | (pack_score << 12) | (pack_depth << 6) | pack_effort;
        let start = bucket_idx * CACHE_WAYS;
        let mut replace = start;
        for idx in start..start + CACHE_WAYS {
            let entry = self.entries[idx];
            if entry == 0 {
                replace = idx;
                continue;
            }
            if (entry >> 32) as u32 == sig {
                if depth > ((entry >> 6) & 0x3f) as i32 {
                    self.entries[idx] = new_entry;
                }
                return;
            }
            if (entry & 0x3f) < (self.entries[replace] & 0x3f) {
                replace = idx;
            }
        }
        self.entries[replace] = new_entry;
    }
}

pub(crate) fn choose_move(board: Board) -> MoveResult {
    let Some(tablebase) = tablebase() else {
        return MoveResult {
            algorithm: AlgorithmId::EndgameTablebase,
            move_id: -1,
            depth: 0,
            nodes: 0,
            cache_hits: 0,
        };
    };

    CORE_AI.with(|core| {
        let mut core = core.borrow_mut();
        core.calculate_step(tablebase, reverse_nibbles(board))
    })
}

fn node_budget(depth: i32, time_limit: f64) -> u64 {
    let base = 20_000u64.saturating_mul(depth.max(1) as u64);
    let scale = if time_limit <= 0.11 {
        1
    } else if time_limit <= 0.33 {
        4
    } else if time_limit <= 0.7 {
        8
    } else {
        16
    };
    let scaled = base.saturating_mul(scale);
    scaled.clamp(30_000, 3_000_000)
}

fn eval_tables() -> &'static [ScorePair; 65536] {
    EVAL_TABLES.get_or_init(|| {
        let mut tables = Box::new([ScorePair { d1: 0, d2: 0 }; 65536]);
        for row in 0..65536usize {
            let mut line = [0i32; 4];
            let mut line_rev = [0i32; 4];
            for i in 0..4 {
                let rank = (row >> (i * 4)) & 0xf;
                let weight = TILE_WEIGHT_MAP[rank];
                line[i] = weight;
                line_rev[3 - i] = weight;
            }
            tables[row] = ScorePair {
                d1: diffs_evaluation_func(line) as i16,
                d2: diffs_evaluation_func(line_rev) as i16,
            };
        }
        tables
    })
}

fn default_eval_overrides() -> [EvalOverride; 8] {
    [
        eval_override(0x78ff, 0xff87),
        eval_override(0x7fff, 0xfff7),
        eval_override(0x8fff, 0xfff8),
        eval_override(0x2fff, 0xfff2),
        eval_override(0x3fff, 0xfff3),
        eval_override(0x4fff, 0xfff4),
        eval_override(0x5fff, 0xfff5),
        eval_override(0x6fff, 0xfff6),
    ]
}

fn eval_override(index: usize, index2: usize) -> EvalOverride {
    EvalOverride {
        index,
        index2,
        score: row_score(index),
    }
}

fn row_score(row: usize) -> i16 {
    let mut line = [0i32; 4];
    for (i, cell) in line.iter_mut().enumerate() {
        let rank = (row >> (i * 4)) & 0xf;
        *cell = TILE_WEIGHT_MAP[rank];
    }
    diffs_evaluation_func(line) as i16
}

fn target_bonus(index: usize) -> i16 {
    match index {
        0x78ff => 300,
        0x7fff => 320,
        0x8fff => 360,
        _ => 0,
    }
}

fn dynamic_target_bonuses(index: usize) -> (i16, i16) {
    match index {
        0x2fff => (4, -4),
        0x3fff => (12, -10),
        0x4fff => (18, -16),
        0x5fff => (24, -20),
        0x6fff => (25, -8),
        _ => (0, 0),
    }
}

fn diffs_evaluation_func(line: [i32; 4]) -> i32 {
    let mut score_dpdf = line[0];
    for x in 0..3 {
        if line[x] < line[x + 1] {
            if line[x] > 400 {
                score_dpdf += (line[x] << 1) + (line[x + 1] - line[x]) * x as i32;
            } else if line[x] > 300 && x == 1 && line[0] > line[1] {
                score_dpdf += line[x] << 1;
            } else {
                score_dpdf -= (line[x + 1] - line[x]) << 3;
                score_dpdf -= line[x + 1] * 3;
                if x < 2 && line[x + 2] < line[x + 1] && line[x + 1] > 30 {
                    score_dpdf -= 80.max(line[x + 1]);
                }
            }
        } else if x < 2 {
            score_dpdf += line[x + 1] + line[x];
        } else {
            score_dpdf += (line[x + 1] + line[x]) / 2;
        }
    }
    if line[0] > 400 && line[1] > 300 && line[2] > 200 && line[2] > line[3] && line[3] < 300 {
        score_dpdf += line[3] >> 2;
    }

    let min_03 = line[0].min(line[3]);
    let mut score_t = if min_03 < 32 {
        -16384
    } else if (line[0] < line[1] && line[0] < 400) || (line[3] < line[2] && line[3] < 400) {
        -(line[1].max(line[2]) * 10)
    } else {
        let mut score = (line[0] * 18 + line[3] * 18) / 10
            + line[1].max(line[2]) * 3 / 2
            + 160.min(line[1].min(line[2])) * 5 / 2;
        if line[1].min(line[2]) < 8 {
            score -= 60;
        }
        score
    };
    let zero_count = line.into_iter().filter(|&v| v == 0).count();
    let sum_123 = line[1] + line[2] + line[3];
    let penalty =
        i32::from(line[0] > 100 && ((zero_count > 1 && sum_123 < 32) || sum_123 < 12)) * 4;
    score_t = score_t.max(score_dpdf);
    score_t / 4 - penalty
}

fn process_score(score: u32) -> i32 {
    let s = score as i32;
    if s < 200 {
        0.max((s >> 2) - 10)
    } else if s < 500 {
        (s >> 1) - 12
    } else if s < 1000 {
        (s >> 1) + 144
    } else if s < 2000 {
        s + 600
    } else {
        3000
    }
}

fn get_max_layer(spawnrate: f64, depth: i32) -> i32 {
    let depth_i = depth.max(0);
    let depth = f64::from(depth_i);
    let variance = (depth * spawnrate * (1.0 - spawnrate)).sqrt();
    let layer = ceil_small_i32(depth * (1.0 + spawnrate) + 3.72 * variance);
    (depth_i * 2).min(layer)
}

fn ceil_small_i32(value: f64) -> i32 {
    let mut out = 0;
    while f64::from(out) < value {
        out += 1;
    }
    out
}

fn round_small_i32(value: f64) -> i32 {
    ceil_small_i32(value + 0.5) - 1
}

fn empty_cell_mask(board: Board) -> Board {
    let mut x = board | (board >> 1);
    x |= x >> 2;
    (!x) & 0x1111_1111_1111_1111
}

fn move_score(board: Board, internal_move: usize) -> u32 {
    let local = reverse_nibbles(board);
    match internal_move {
        1 | 2 => {
            let mut score = 0;
            for r in 0..4 {
                let mut line = [0u8; 4];
                for c in 0..4 {
                    line[c] = ((local >> ((r * 4 + c) * 4)) & 0xf) as u8;
                }
                if internal_move == 2 {
                    line.reverse();
                }
                score += merge_line_score(line);
            }
            score
        }
        3 | 4 => {
            let mut score = 0;
            for c in 0..4 {
                let mut line = [0u8; 4];
                for r in 0..4 {
                    line[r] = ((local >> ((r * 4 + c) * 4)) & 0xf) as u8;
                }
                if internal_move == 4 {
                    line.reverse();
                }
                score += merge_line_score(line);
            }
            score
        }
        _ => 0,
    }
}

fn merge_line_score(line: [u8; 4]) -> u32 {
    let mut non_zero = [0u8; 4];
    let mut n = 0;
    for rank in line {
        if rank != 0 {
            non_zero[n] = rank;
            n += 1;
        }
    }
    let mut score = 0;
    let mut i = 0;
    while i < n {
        if i + 1 < n && non_zero[i] == non_zero[i + 1] && non_zero[i] != 0xf {
            score += 1u32 << (non_zero[i] + 1);
            i += 2;
        } else {
            i += 1;
        }
    }
    score
}

fn large_tile_count(start: usize, counts: &[u8; 16]) -> i32 {
    let mut i = start;
    while i < 16 && counts[i] != 0 {
        i += 1;
    }
    let mut sum = 0;
    i += 1;
    while i < 16 {
        if counts[i] == 2 && i < 15 {
            return 0;
        }
        sum += i32::from(counts[i]);
        i += 1;
    }
    sum
}

fn is_5tiler(board_sum: i32, counts: &[u8; 16]) -> bool {
    let sum_range = board_sum > 62000 && board_sum < 65520;
    let counts_cond = sum_counts(counts, 11, 16) == 4 && counts[10] == 0;
    let rem = board_sum % 1024;
    (sum_range || counts_cond) && (rem < 24 || rem > 996)
}

fn tiles_all_set(counts: &[u8; 16]) -> bool {
    let mut last_dup = 0;
    for (rank, &count) in counts.iter().enumerate().take(15).skip(3) {
        if count > 1 {
            last_dup = rank;
        }
    }
    if last_dup == 0 {
        return false;
    }
    let mut i = last_dup + 1;
    while i < 15 && counts[i] != 0 {
        i += 1;
    }
    let final_big_tiles = sum_counts(counts, i, 16) + 1;
    final_big_tiles < 5 && i > 9 && !(final_big_tiles + (i as u16) < 14 && last_dup < 6)
}

fn get_23_mask(board: Board) -> Board {
    first_matching_mask(
        board,
        &[
            0xff00fff0,
            0xfff00ff00000000,
            0xf00ff00ff,
            0xff00ff00f0000000,
            0xfff0ff0000000000,
            0xff00ff000f0000,
            0xf000ff00ff00,
            0xff0fff,
        ],
    )
}

fn get_22_mask(board: Board) -> Board {
    first_matching_mask(
        board,
        &[
            0xff00ff0000000000,
            0x00ff00ff00000000,
            0x00000000ff00ff00,
            0x0000000000ff00ff,
        ],
    )
}

fn get_21_mask(board: Board) -> Board {
    first_matching_mask(
        board,
        &[
            0xff00f00000000000,
            0x00ff000f00000000,
            0x00000000f000ff00,
            0x00000000000f00ff,
        ],
    )
}

fn get_20_mask(board: Board) -> Board {
    first_matching_mask(
        board,
        &[
            0xff00,
            0xff000000000000,
            0xf000f,
            0xf000f00000000000,
            0xff00000000000000,
            0xf000f00000000,
            0xf000f000,
            0xff,
        ],
    )
}

fn first_matching_mask(board: Board, masks: &[Board]) -> Board {
    masks
        .iter()
        .copied()
        .find(|&mask| (board & mask) == mask)
        .unwrap_or(0)
}

fn check_corner_penalty(board: Board, masked_count: i32) -> i32 {
    const M6_NEG600: &[Board] = &[
        0xf000fff0ff,
        0xff0fff000f000000,
        0xff0fff0000000f,
        0xf0000000fff0ff00,
        0xf0ff00ff00f00000,
        0xf00000fff00ff,
        0xff00fff00000f000,
        0xf00ff00ff0f,
    ];
    const M6_NEG500: &[Board] = &[
        0xf0000000ff0fff00,
        0xfff0ff0000000f,
        0xf0000000fff0ff,
        0xff0fff0000000f00,
        0xff00ff0f0000f000,
        0xf0ff00ff000000f0,
        0xf000000ff00ff0f,
        0xf0000f0ff00ff,
    ];
    const M6_NEG1600: &[Board] = &[
        0xf00000fff0ff,
        0xff0fff00000f0000,
        0xff00ff00000f0f,
        0xf0f00000ff00ff00,
        0xf0ff00fff0000000,
        0xf0f000000ff00ff,
        0xff00ff000000f0f0,
        0xfff00ff0f,
    ];
    const M6_NEG2400: &[Board] = &[
        0xff0fff0f,
        0xf0fff0ff00000000,
        0xff000000ff00ff,
        0xff00ff000000ff00,
        0xff0fff0f00000000,
        0xff00ff000000ff,
        0xff000000ff00ff00,
        0xf0fff0ff,
    ];
    const M6_POS600: &[Board] = &[
        0xffff0ff,
        0xff0ffff000000000,
        0xff00ff00f0000f,
        0xf0000f00ff00ff00,
        0xf0ff0fff00000000,
        0xf00f000ff00ff,
        0xff00ff000f00f000,
        0xfff0ff0f,
    ];
    const M4_POS3000: &[Board] = &[
        0xff00f00f,
        0xff00f00000000f,
        0xfff00f,
        0xff000f000000f000,
        0xf00000000f00ff00,
        0xf00f00ff00000000,
        0xf00fff0000000000,
        0xf000000f000ff,
    ];
    const M4_POS800: &[Board] = &[
        0xf000f000f00f,
        0xf000ff00f,
        0xfff000000000f000,
        0xf00000000000fff0,
        0xf00f000f000f0000,
        0xf00ff000f0000000,
        0xf000000000fff,
        0xfff00000000000f,
    ];
    const M4_POS1600: &[Board] = &[
        0xff000000f000f000,
        0xf000f000000ff,
        0xf000f0ff,
        0xff0f000f00000000,
        0xf000f0000000ff00,
        0xf0fff00000000000,
        0xfff0f,
        0xff0000000f000f,
    ];
    const M3_POS3000: &[Board] = &[
        0xf00000000f00f,
        0xf00f00000000000f,
        0xf00000000000f00f,
        0xf00f00000000f000,
    ];
    const M3_POS2000: &[Board] = &[
        0xf0000000000f00f,
        0xf000000000f00f,
        0xf0000000000ff000,
        0xf000000f0000f000,
        0xf00f0000000000f0,
        0xf00f000000000f00,
        0xf0000f000000f,
        0xff0000000000f,
    ];
    const M3_POS1000: &[Board] = &[
        0xff00f,
        0xf000f00f,
        0xf00000000000ff00,
        0xff0000000000f000,
        0xf00ff00000000000,
        0xf00f000f00000000,
        0xff00000000000f,
        0xf0000000000ff,
    ];

    if masked_count == 6 {
        if any_mask(board, M6_NEG600) {
            return -600;
        }
        if any_mask(board, M6_NEG500) {
            return -500;
        }
        if any_mask(board, M6_NEG1600) {
            return -1600;
        }
        if any_mask(board, M6_NEG2400) {
            return -2400;
        }
        if any_mask(board, M6_POS600) {
            return 600;
        }
    }
    if masked_count == 4 {
        if any_mask(board, M4_POS3000) {
            return 3000;
        }
        if any_mask(board, M4_POS800) {
            return 800;
        }
        if any_mask(board, M4_POS1600) {
            return 1600;
        }
    }
    if masked_count == 3 || masked_count == 4 {
        if any_mask(board, M3_POS3000) {
            return 3000;
        }
        if any_mask(board, M3_POS2000) {
            return 2000;
        }
    }
    if masked_count == 3 && any_mask(board, M3_POS1000) {
        return 1000;
    }
    0
}

fn any_mask(board: Board, masks: &[Board]) -> bool {
    masks.iter().any(|&mask| (board & mask) == mask)
}

fn is_mess(board: Board, board_sum: i32) -> bool {
    if board_sum % 512 < 12 {
        return false;
    }
    let mut tiles = [(0i32, 0usize); 16];
    let mut tmp = board;
    let mut large_tiles = 0;
    for (idx, tile) in tiles.iter_mut().enumerate() {
        let rank = (tmp & 0xf) as i32;
        let value = if rank == 0 { 0 } else { 1 << rank };
        *tile = (value, idx);
        if value > 128 {
            large_tiles += 1;
        }
        tmp >>= 4;
    }
    if large_tiles < 3 {
        return false;
    }
    tiles.sort_by(|a, b| b.0.cmp(&a.0));
    match large_tiles {
        6 => !positions_allowed(&tiles, 6, &MESS_ALLOWED_6),
        4 => !positions_allowed(&tiles, 4, &MESS_ALLOWED_4),
        3 => {
            if tiles[0].0 == tiles[1].0 || tiles[1].0 == tiles[2].0 {
                false
            } else {
                !positions_allowed(&tiles, 3, &MESS_ALLOWED_3)
            }
        }
        _ => {
            let mut pos_mask = 0u32;
            for &(_, idx) in tiles.iter().take(large_tiles as usize) {
                pos_mask |= 1 << idx;
            }
            ![[0, 1, 4], [3, 2, 7], [12, 8, 13], [15, 11, 14]]
                .iter()
                .any(|shape| shape.iter().all(|&idx| pos_mask & (1 << idx) != 0))
        }
    }
}

fn positions_allowed(tiles: &[(i32, usize); 16], count: usize, allowed: &[u32]) -> bool {
    let mut mask = 0u32;
    for &(_, idx) in tiles.iter().take(count) {
        mask |= 1 << idx;
    }
    allowed.contains(&mask)
}

const MESS_ALLOWED_6: [u32; 4] = [
    bits(&[0, 1, 2, 3, 4, 5]),
    bits(&[0, 1, 2, 3, 6, 7]),
    bits(&[8, 9, 12, 13, 14, 15]),
    bits(&[10, 11, 12, 13, 14, 15]),
];

const MESS_ALLOWED_4: [u32; 24] = [
    bits(&[0, 1, 2, 3]),
    bits(&[0, 4, 8, 12]),
    bits(&[12, 13, 14, 15]),
    bits(&[3, 7, 11, 15]),
    bits(&[0, 1, 2, 4]),
    bits(&[4, 8, 12, 13]),
    bits(&[11, 13, 14, 15]),
    bits(&[2, 3, 7, 11]),
    bits(&[0, 1, 4, 8]),
    bits(&[8, 12, 13, 14]),
    bits(&[7, 11, 14, 15]),
    bits(&[1, 2, 3, 7]),
    bits(&[0, 1, 4, 5]),
    bits(&[8, 9, 12, 13]),
    bits(&[10, 11, 14, 15]),
    bits(&[2, 3, 6, 7]),
    bits(&[0, 1, 3, 4]),
    bits(&[0, 1, 4, 12]),
    bits(&[0, 2, 3, 7]),
    bits(&[2, 3, 7, 15]),
    bits(&[0, 8, 12, 13]),
    bits(&[8, 12, 13, 15]),
    bits(&[3, 11, 14, 15]),
    bits(&[11, 12, 14, 15]),
];

const MESS_ALLOWED_3: [u32; 20] = [
    bits(&[0, 1, 2]),
    bits(&[1, 2, 3]),
    bits(&[3, 7, 11]),
    bits(&[7, 11, 15]),
    bits(&[13, 14, 15]),
    bits(&[12, 13, 14]),
    bits(&[4, 8, 12]),
    bits(&[0, 4, 8]),
    bits(&[0, 1, 3]),
    bits(&[0, 2, 3]),
    bits(&[3, 7, 15]),
    bits(&[3, 11, 15]),
    bits(&[12, 14, 15]),
    bits(&[12, 13, 15]),
    bits(&[0, 8, 12]),
    bits(&[0, 4, 12]),
    bits(&[0, 1, 4]),
    bits(&[2, 3, 7]),
    bits(&[11, 14, 15]),
    bits(&[8, 12, 13]),
];

const fn bits(values: &[usize]) -> u32 {
    let mut out = 0;
    let mut i = 0;
    while i < values.len() {
        out |= 1 << values[i];
        i += 1;
    }
    out
}

fn danbianhuichuan_patch(_board: Board, _board_sum: i32) -> bool {
    false
}

fn tablebase() -> Option<&'static Tablebase> {
    TABLEBASE
        .get_or_init(|| parse_tablebase(TABLE_BYTES).ok())
        .as_ref()
}

fn parse_tablebase(bytes: &'static [u8]) -> Result<Tablebase, &'static str> {
    if bytes.len() < 12 || &bytes[..8] != TABLE_MAGIC {
        return Err("bad tablebase magic");
    }

    let mut pos = 8;
    let table_count = read_u32(bytes, &mut pos)? as usize;
    let mut tables = Vec::with_capacity(table_count);
    for _ in 0..table_count {
        let table_type = read_u32(bytes, &mut pos)?;
        let layer_count = read_u32(bytes, &mut pos)? as usize;
        let initial_sum = read_i32(bytes, &mut pos)?;
        let lower = read_f32(bytes, &mut pos)?;
        let upper = read_f32(bytes, &mut pos)?;
        let flags = read_u32(bytes, &mut pos)?;
        let mut layers = Vec::with_capacity(layer_count);
        for _ in 0..layer_count {
            let buckets = read_u32(bytes, &mut pos)?;
            let len = read_u32(bytes, &mut pos)?;
            let seed_offset = read_u32(bytes, &mut pos)?;
            let seed_len = read_u32(bytes, &mut pos)?;
            let sig_offset = read_u32(bytes, &mut pos)?;
            let sig_len = read_u32(bytes, &mut pos)?;
            let rate_offset = read_u32(bytes, &mut pos)?;
            let rate_len = read_u32(bytes, &mut pos)?;
            if buckets != seed_len || len != sig_len || len != rate_len {
                return Err("bad tablebase layer lengths");
            }
            layers.push(Layer {
                buckets,
                len,
                seed_offset,
                sig_offset,
                rate_offset,
            });
        }
        tables.push(Table {
            table_type,
            initial_sum,
            lower,
            upper,
            flags,
            layers,
        });
    }

    let blob = &bytes[pos..];
    for table in &tables {
        for layer in &table.layers {
            checked_range(blob, layer.seed_offset, layer.buckets as usize * 2)?;
            checked_range(blob, layer.sig_offset, layer.len as usize)?;
            checked_range(blob, layer.rate_offset, layer.len as usize * 2)?;
        }
    }

    Ok(Tablebase { tables, blob })
}

fn probe(tablebase: &Tablebase, board: Board, counts: [u8; 16], board_sum: i32) -> ProbeResult {
    let c10_15 = sum_counts(&counts, 10, 15);
    let c9_15 = sum_counts(&counts, 9, 15);
    let c8_15 = sum_counts(&counts, 8, 15);
    let c7_15 = sum_counts(&counts, 7, 15);
    let mut threshold = 0u8;

    if c10_15 == 6 && max_counts(&counts, 10, 15) == 1 {
        if !((board_sum % 1024) < 480 && counts[9] == 1) {
            if board_sum % 1024 > 96 {
                threshold = 10;
            } else if board_sum % 1024 > 60 {
                threshold = 8;
            }
        }
    } else if c9_15 == 6 && max_counts(&counts, 9, 15) == 1 {
        if !((board_sum % 512) < 240 && counts[8] == 1) {
            threshold = 9;
        }
    } else if c8_15 == 6 && max_counts(&counts, 8, 15) == 1 && (board_sum % 256) < 240 {
        if !((board_sum % 256) < 120 && counts[7] == 1)
            && !(sum_counts(&counts, 10, 16) == 5 && counts[9] == 0 && (board_sum % 256) < 64)
        {
            threshold = 8;
        }
    } else if c7_15 == 6
        && max_counts(&counts, 7, 15) == 1
        && 20 < (board_sum % 128)
        && (board_sum % 128) < 120
        && counts[7] == 1
        && !((board_sum % 128) < 60 && counts[6] == 1)
    {
        threshold = 7;
    }

    if threshold == 0 {
        return empty_probe();
    }

    let masked = mask_large_tiles(board, threshold, 0xf);
    let table_types = table_types_for(threshold, board_sum);
    let mut result = probe_l3(tablebase, masked, &table_types, board_sum);
    result.threshold = threshold;

    if result.move_id == 0
        && c8_15 + u16::from(counts[15]) == 7
        && max_counts(&counts, 8, 15) == 1
        && (board_sum % 256) < 240
    {
        threshold = 9;
        result = probe_441(tablebase, board, threshold - 1, board_sum);
        result.threshold = threshold;
        result.table_type = 512;
    }
    if result.move_id == 0
        && c7_15 + u16::from(counts[15]) == 7
        && max_counts(&counts, 7, 15) == 1
        && (board_sum % 128) < 120
    {
        threshold = 8;
        result = probe_441(tablebase, board, threshold - 1, board_sum);
        result.threshold = threshold;
        result.table_type = 512;
    }

    if threshold <= 8 && max_rate(result.rates) > 0.0 && max_rate(result.rates) < 0.625 {
        result.move_id = 0;
        result.is_evil = true;
        result.table_type = 0;
    }
    result
}

fn probe_l3(
    tablebase: &Tablebase,
    mut masked_board: Board,
    table_types: &[u32],
    board_sum: i32,
) -> ProbeResult {
    let mut local_types = [0u32; 2];
    let mut types = table_types;
    if 65280 < board_sum && board_sum < 65436 {
        masked_board = mask_large_tiles(masked_board, 8, 0xf);
        local_types[0] = 1256;
        types = &local_types[..1];
    } else if 65000 < board_sum && board_sum < 65280 {
        masked_board = mask_large_tiles(masked_board, 9, 0x8);
        local_types[0] = 512;
        types = &local_types[..1];
    }

    let syms = symmetries(masked_board);
    let mut queries = 0;
    for (sym_idx, &sym) in syms.iter().enumerate() {
        let board = sym;
        if (board & 0x0fff_0fff) != 0x0fff_0fff {
            continue;
        }

        for &table_type in types {
            if table_type == 512 {
                let mut res = probe_44_128(tablebase, board, sym_idx, board_sum);
                queries += res.queries;
                if res.move_id != 0 {
                    res.queries = queries;
                    return res;
                }
            }

            let rates = find_best_egtb_move(tablebase, board, table_type, &mut queries);
            if max_rate(rates) > 0.0 {
                let (move_id, original_rates) = handle_result(rates, sym_idx);
                return ProbeResult {
                    move_id,
                    is_evil: false,
                    table_type,
                    rates: original_rates,
                    threshold: 0,
                    queries,
                };
            }
        }
    }
    let mut out = empty_probe();
    out.queries = queries;
    out
}

fn probe_441(tablebase: &Tablebase, board: Board, threshold: u8, board_sum: i32) -> ProbeResult {
    let masked = mask_large_tiles(board, threshold, threshold);
    let syms = symmetries(masked);
    let thresh = u64::from(threshold);
    let mut queries = 0;
    for (sym_idx, &sym) in syms.iter().enumerate() {
        let mut board = sym;
        if (board & 0x0fff_0fff) == (0x111_0111 * thresh) {
            board |= 0x0fff_0fff;
        } else {
            continue;
        }

        let mut res = probe_44_128(tablebase, board, sym_idx, board_sum);
        queries += res.queries;
        if res.move_id != 0 {
            res.queries = queries;
            return res;
        }

        let rates = find_best_egtb_move(tablebase, board, 512, &mut queries);
        if max_rate(rates) > 0.0 {
            let (move_id, original_rates) = handle_result(rates, sym_idx);
            return ProbeResult {
                move_id,
                is_evil: false,
                table_type: 512,
                rates: original_rates,
                threshold: 0,
                queries,
            };
        }
    }
    let mut out = empty_probe();
    out.queries = queries;
    out
}

fn probe_44_128(
    tablebase: &Tablebase,
    masked_board: Board,
    sym_idx: usize,
    board_sum: i32,
) -> ProbeResult {
    let mut queries = 0;
    if 390 < (board_sum % 512) && (board_sum % 512) < 480 && board_sum > 63000 {
        let mut remask = mask_large_tiles(masked_board, 7, 0xf);
        if (remask & 0xffff_ffff) != 0xffff_ffff {
            return empty_probe();
        }
        remask = (reverse_lr(remask) & 0xffff_ffff_0000_0000) + 0x7fff_8fff;
        let mut rates = find_best_egtb_move(tablebase, remask, 512, &mut queries);
        if max_rate(rates) > 0.0 {
            rates.swap(0, 1);
            let (move_id, original_rates) = handle_result(rates, sym_idx);
            return ProbeResult {
                move_id,
                is_evil: false,
                table_type: 512,
                rates: original_rates,
                threshold: 0,
                queries,
            };
        }
    }
    let mut out = empty_probe();
    out.queries = queries;
    out
}

fn find_best_egtb_move(
    tablebase: &Tablebase,
    target_board: Board,
    table_type: u32,
    queries: &mut u64,
) -> [f32; 4] {
    let mut rates = [0.0; 4];
    if target_board == 0x0124_1256_7fff_8fff {
        rates[0] = 0.9111;
        return rates;
    }

    let Some(table) = tablebase
        .tables
        .iter()
        .find(|table| table.table_type == table_type)
    else {
        return rates;
    };

    for internal_move in 1..=4 {
        let post_board = execute_internal_move(target_board, internal_move);
        if post_board == target_board {
            continue;
        }

        let board_sum = get_board_sum(post_board) % 32768;
        let layer = (board_sum - table.initial_sum) / 2;
        let query_board = if table.flags & DIAGONAL_CANONICAL_FLAG != 0 {
            canonical_diagonal(post_board)
        } else {
            post_board
        };
        let compressed = compress_board(query_board);
        if compressed == 0 {
            continue;
        }

        *queries += 1;
        if let Some(rate) = query_perfect_hash(tablebase, table, compressed, layer) {
            rates[internal_move - 1] =
                (rate as f32 / 65535.0) * (table.upper - table.lower) + table.lower;
        }
    }

    rates
}

fn query_perfect_hash(tablebase: &Tablebase, table: &Table, b: u32, layer_idx: i32) -> Option<u16> {
    let layer = table.layers.get(layer_idx as usize)?;
    if layer.buckets == 0 || layer.len == 0 {
        return None;
    }
    let bucket = mix32(b) % layer.buckets;
    let seed = read_u16_at(
        tablebase.blob,
        layer.seed_offset as usize + bucket as usize * 2,
    )?;
    let hash_idx = mix32(b ^ u32::from(seed)) % layer.len;
    let sig = *tablebase
        .blob
        .get(layer.sig_offset as usize + hash_idx as usize)?;
    let expected = ((b + (b >> 8) + (b >> 16) + (b >> 24)) & 0xff) as u8;
    if sig == expected {
        read_u16_at(
            tablebase.blob,
            layer.rate_offset as usize + hash_idx as usize * 2,
        )
    } else {
        None
    }
}

fn probe_after_move_runtime(
    tablebase: &Tablebase,
    board: Board,
    threshold: u8,
    table_types: &[u32],
    board_sum: i32,
) -> f32 {
    let masked = mask_large_tiles(board, threshold, 0xf);
    let mut win_rate = 0.0;
    let mut empty_slots = 0;
    for pos in 0..16 {
        let shift = pos * 4;
        if ((masked >> shift) & 0xf) == 0 {
            empty_slots += 1;
            let t1 = masked | (1u64 << shift);
            let t2 = masked | (2u64 << shift);
            win_rate += max_rate(probe_l3(tablebase, t1, table_types, board_sum + 2).rates) * 0.9;
            win_rate += max_rate(probe_l3(tablebase, t2, table_types, board_sum + 4).rates) * 0.1;
        }
    }
    if empty_slots == 0 {
        0.0
    } else {
        win_rate / empty_slots as f32
    }
}

#[cfg(test)]
fn probe_after_move(
    tablebase: &Tablebase,
    board: Board,
    threshold: u8,
    table_types: &[u32],
    board_sum: i32,
) -> f32 {
    probe_after_move_runtime(tablebase, board, threshold, table_types, board_sum)
}

fn table_types_for(threshold: u8, board_sum: i32) -> [u32; 2] {
    if threshold == 10 && (board_sum % 1024) < 128 {
        [256, 512]
    } else if threshold > 8 && ((board_sum % 256) > 128 || (board_sum % 512) < 72) {
        [512, 0]
    } else if threshold > 8 {
        [512, 256]
    } else if (threshold == 8 && (board_sum % 256) > 60)
        || (threshold == 7 && (board_sum % 128) > 60)
    {
        [256, 512]
    } else {
        [256, 0]
    }
}

fn handle_result(rates: [f32; 4], sym_idx: usize) -> (usize, [f32; 4]) {
    let found_dir = argmax(rates) + 1;
    let best_move = MAP_MOVE[sym_idx][found_dir];
    let mut original_rates = [0.0; 4];
    for direction in 1..5 {
        let original_dir = MAP_MOVE[sym_idx][direction];
        original_rates[original_dir - 1] = rates[direction - 1];
    }
    (best_move, original_rates)
}

fn empty_probe() -> ProbeResult {
    ProbeResult {
        move_id: 0,
        is_evil: false,
        table_type: 0,
        rates: [0.0; 4],
        threshold: 0,
        queries: 0,
    }
}

fn execute_internal_move(board: Board, internal_move: usize) -> Board {
    let local = reverse_nibbles(board);
    let moved = execute_move(internal_to_api_move(internal_move), local);
    reverse_nibbles(moved)
}

fn legal_internal_move(board: Board, internal_move: usize) -> bool {
    execute_internal_move(board, internal_move) != board
}

fn legalize_internal_move(board: Board, internal_move: usize) -> usize {
    if (1..=4).contains(&internal_move) && legal_internal_move(board, internal_move) {
        return internal_move;
    }
    (1..=4)
        .find(|&candidate| legal_internal_move(board, candidate))
        .unwrap_or(0)
}

fn internal_to_api_move(internal_move: usize) -> i32 {
    match internal_move {
        1 => 2,
        2 => 3,
        3 => 0,
        4 => 1,
        _ => -1,
    }
}

fn mask_large_tiles(mut board: Board, threshold: u8, mask: u8) -> Board {
    let mut out = 0;
    for pos in 0..16 {
        let mut value = (board & 0xf) as u8;
        if value >= threshold {
            value = mask;
        }
        out |= u64::from(value) << (pos * 4);
        board >>= 4;
    }
    out
}

fn symmetries(board: Board) -> [Board; 8] {
    [
        board,
        rotate_l(board),
        rotate180(board),
        rotate_r(board),
        reverse_lr(board),
        reverse_ud(board),
        reverse_ul(board),
        reverse_ur(board),
    ]
}

fn reverse_lr(mut board: Board) -> Board {
    board = ((board & 0xff00_ff00_ff00_ff00) >> 8) | ((board & 0x00ff_00ff_00ff_00ff) << 8);
    ((board & 0xf0f0_f0f0_f0f0_f0f0) >> 4) | ((board & 0x0f0f_0f0f_0f0f_0f0f) << 4)
}

fn reverse_ud(mut board: Board) -> Board {
    board = ((board & 0xffff_ffff_0000_0000) >> 32) | ((board & 0x0000_0000_ffff_ffff) << 32);
    ((board & 0xffff_0000_ffff_0000) >> 16) | ((board & 0x0000_ffff_0000_ffff) << 16)
}

fn reverse_ul(mut board: Board) -> Board {
    board = (board & 0xff00_ff00_00ff_00ff)
        | ((board & 0x00ff_00ff_0000_0000) >> 24)
        | ((board & 0x0000_0000_ff00_ff00) << 24);
    (board & 0xf0f0_0f0f_f0f0_0f0f)
        | ((board & 0x0f0f_0000_0f0f_0000) >> 12)
        | ((board & 0x0000_f0f0_0000_f0f0) << 12)
}

fn reverse_ur(mut board: Board) -> Board {
    board = (board & 0x0f0f_f0f0_0f0f_f0f0)
        | ((board & 0xf0f0_0000_f0f0_0000) >> 20)
        | ((board & 0x0000_0f0f_0000_0f0f) << 20);
    (board & 0x00ff_00ff_ff00_ff00)
        | ((board & 0xff00_ff00_0000_0000) >> 40)
        | ((board & 0x0000_0000_00ff_00ff) << 40)
}

fn rotate180(board: Board) -> Board {
    reverse_lr(reverse_ud(board))
}

fn rotate_l(mut board: Board) -> Board {
    board = ((board & 0xff00_ff00_0000_0000) >> 32)
        | ((board & 0x00ff_00ff_0000_0000) << 8)
        | ((board & 0x0000_0000_ff00_ff00) >> 8)
        | ((board & 0x0000_0000_00ff_00ff) << 32);
    ((board & 0xf0f0_0000_f0f0_0000) >> 16)
        | ((board & 0x0f0f_0000_0f0f_0000) << 4)
        | ((board & 0x0000_f0f0_0000_f0f0) >> 4)
        | ((board & 0x0000_0f0f_0000_0f0f) << 16)
}

fn rotate_r(mut board: Board) -> Board {
    board = ((board & 0xff00_ff00_0000_0000) >> 8)
        | ((board & 0x00ff_00ff_0000_0000) >> 32)
        | ((board & 0x0000_0000_ff00_ff00) << 32)
        | ((board & 0x0000_0000_00ff_00ff) << 8);
    ((board & 0xf0f0_0000_f0f0_0000) >> 4)
        | ((board & 0x0f0f_0000_0f0f_0000) >> 16)
        | ((board & 0x0000_f0f0_0000_f0f0) << 16)
        | ((board & 0x0000_0f0f_0000_0f0f) << 4)
}

fn canonical_diagonal(board: Board) -> Board {
    board.min(reverse_ul(board))
}

fn reverse_nibbles(mut board: Board) -> Board {
    let mut out = 0;
    for _ in 0..16 {
        out = (out << 4) | (board & 0xf);
        board >>= 4;
    }
    out
}

fn compress_board(board: Board) -> u32 {
    let mut compressed = 0u32;
    for (idx, offset) in BIT_OFFSETS.iter().enumerate() {
        let digit = ((board >> offset) & 0xf) as u32;
        compressed = compressed.wrapping_add(digit.wrapping_mul(POWERS_OF_9[idx]));
    }
    compressed
}

fn get_board_sum(mut board: Board) -> i32 {
    let mut sum = 0;
    for _ in 0..16 {
        let rank = (board & 0xf) as u32;
        if rank > 0 {
            sum += 1i32 << rank;
        }
        board >>= 4;
    }
    sum
}

fn tile_counts(mut board: Board) -> [u8; 16] {
    let mut counts = [0u8; 16];
    for _ in 0..16 {
        counts[(board & 0xf) as usize] += 1;
        board >>= 4;
    }
    counts
}

fn board_sum_from_counts(counts: [u8; 16]) -> i32 {
    let mut sum = 0;
    for (rank, count) in counts.iter().enumerate().skip(1) {
        sum += i32::from(*count) * (1i32 << rank);
    }
    sum
}

fn sum_counts(counts: &[u8; 16], start: usize, end: usize) -> u16 {
    counts[start..end].iter().map(|&v| u16::from(v)).sum()
}

fn max_counts(counts: &[u8; 16], start: usize, end: usize) -> u8 {
    counts[start..end].iter().copied().max().unwrap_or(0)
}

fn max_rate(rates: [f32; 4]) -> f32 {
    rates.into_iter().fold(0.0, f32::max)
}

fn argmax(values: [f32; 4]) -> usize {
    let mut best_idx = 0;
    let mut best = values[0];
    for (idx, value) in values.into_iter().enumerate().skip(1) {
        if value > best {
            best = value;
            best_idx = idx;
        }
    }
    best_idx
}

fn argmax_i32(values: [i32; 4]) -> usize {
    let mut best_idx = 0;
    let mut best = values[0];
    for (idx, value) in values.into_iter().enumerate().skip(1) {
        if value > best {
            best = value;
            best_idx = idx;
        }
    }
    best_idx
}

fn sorted_desc(mut values: [i32; 4]) -> [i32; 4] {
    values.sort_unstable_by(|a, b| b.cmp(a));
    values
}

fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca77);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2_ae3d);
    x ^ (x >> 16)
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, &'static str> {
    let value = u32::from_le_bytes(read_array(bytes, pos)?);
    Ok(value)
}

fn read_i32(bytes: &[u8], pos: &mut usize) -> Result<i32, &'static str> {
    let value = i32::from_le_bytes(read_array(bytes, pos)?);
    Ok(value)
}

fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32, &'static str> {
    let value = f32::from_le_bytes(read_array(bytes, pos)?);
    Ok(value)
}

fn read_array<const N: usize>(bytes: &[u8], pos: &mut usize) -> Result<[u8; N], &'static str> {
    let end = pos.checked_add(N).ok_or("tablebase offset overflow")?;
    let slice = bytes.get(*pos..end).ok_or("truncated tablebase")?;
    *pos = end;
    slice.try_into().map_err(|_| "bad tablebase read")
}

fn read_u16_at(bytes: &[u8], pos: usize) -> Option<u16> {
    let slice = bytes.get(pos..pos + 2)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

fn checked_range(bytes: &[u8], offset: u32, len: usize) -> Result<(), &'static str> {
    let start = offset as usize;
    let end = start.checked_add(len).ok_or("tablebase range overflow")?;
    if end <= bytes.len() {
        Ok(())
    } else {
        Err("tablebase range out of bounds")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn max_layer_matches_cpp_formula() {
        let cases = [
            (1, 2),
            (2, 4),
            (3, 6),
            (4, 7),
            (5, 8),
            (6, 10),
            (8, 12),
            (10, 15),
            (12, 18),
            (24, 32),
            (48, 61),
        ];
        for (depth, expected) in cases {
            assert_eq!(get_max_layer(0.1, depth), expected);
        }
    }

    #[test]
    fn dynamic_eval_overrides_match_cpp_targets() {
        let board_with_corner = 0xff00_ff00_0000_0000;
        let board_without_corner = 0;
        let mut ai = AiPlayer::new();

        ai.update_specific_scores(board_without_corner, true, true, true, true);
        assert_eq!(ai.score_pair(0x78ff).d1, row_score(0x78ff));
        assert_eq!(ai.score_pair(0xff87).d2, row_score(0x78ff));
        assert_eq!(ai.score_pair(0x2fff).d1, row_score(0x2fff));
        assert_eq!(ai.score_pair(0xfff2).d2, row_score(0x2fff));

        ai.update_specific_scores(board_with_corner, false, false, true, false);
        assert_eq!(ai.score_pair(0x78ff).d1, row_score(0x78ff) + 300);
        assert_eq!(ai.score_pair(0xff87).d2, row_score(0x78ff) + 300);
        assert_eq!(ai.score_pair(0x7fff).d1, row_score(0x7fff) + 320);
        assert_eq!(ai.score_pair(0x8fff).d1, row_score(0x8fff) + 360);

        ai.update_specific_scores(board_with_corner, true, false, false, false);
        assert_eq!(ai.score_pair(0x2fff).d1, row_score(0x2fff) + 4);
        assert_eq!(ai.score_pair(0xfff2).d2, row_score(0x2fff) + 4);
        assert_eq!(ai.score_pair(0x6fff).d1, row_score(0x6fff) + 25);

        ai.update_specific_scores(board_with_corner, false, true, false, false);
        assert_eq!(ai.score_pair(0x3fff).d1, row_score(0x3fff) - 10);
        assert_eq!(ai.score_pair(0xfff3).d2, row_score(0x3fff) - 10);
        assert_eq!(ai.score_pair(0x6fff).d1, row_score(0x6fff) - 8);

        ai.update_specific_scores(board_with_corner, false, false, false, true);
        assert_eq!(ai.score_pair(0x4fff).d1, row_score(0x4fff) + 9);
        assert_eq!(ai.score_pair(0xfff4).d2, row_score(0x4fff) + 9);
        assert_eq!(ai.score_pair(0x5fff).d1, row_score(0x5fff) + 12);
    }

    #[test]
    fn cache_packs_effort_and_replaces_lowest_effort_entry() {
        let mut cache = SearchCache::new();
        cache.reset(1);
        for board in 1..=8 {
            cache.update(0, board, 3, board as i32, 131072, 1 << (board + 3));
        }

        let start = 0;
        assert!(cache.entries[start..start + CACHE_WAYS]
            .iter()
            .any(|entry| (entry & 0x3f) == 4));

        cache.update(0, 99, 3, 99, 131072, 1 << 20);
        assert!(cache.entries[start..start + CACHE_WAYS]
            .iter()
            .any(|entry| (entry & 0x3f) == 20));
        assert!(!cache.entries[start..start + CACHE_WAYS]
            .iter()
            .any(|entry| (entry & 0x3f) == 4));
    }

    #[test]
    fn choose_move_returns_legal_api_direction_for_endgame_sample() {
        let board = 0x0000_1234_5678_9abc;
        let result = choose_move(board);
        assert!((-1..4).contains(&result.move_id));
        if result.move_id >= 0 {
            assert_ne!(execute_move(result.move_id, board), board);
        }
    }

    pub(crate) fn bundled_tablebase_parses() -> bool {
        parse_tablebase(TABLE_BYTES).is_ok()
    }

    pub(crate) fn bad_magic_is_rejected() -> bool {
        parse_tablebase(b"badmagic").is_err()
    }

    pub(crate) fn special_case_move_is_left() -> bool {
        let tablebase = parse_tablebase(TABLE_BYTES).expect("bundled tablebase");
        let mut queries = 0;
        let rates = find_best_egtb_move(&tablebase, 0x0124_1256_7fff_8fff, 512, &mut queries);
        argmax(rates) + 1 == 1
    }

    pub(crate) fn direction_mapping_is_stable() -> bool {
        internal_to_api_move(1) == 2
            && internal_to_api_move(2) == 3
            && internal_to_api_move(3) == 0
            && internal_to_api_move(4) == 1
    }

    pub(crate) fn probe_after_move_runs() -> bool {
        let tablebase = parse_tablebase(TABLE_BYTES).expect("bundled tablebase");
        probe_after_move(&tablebase, 0x0124_1256_7fff_8fff, 8, &[512], 65000) >= 0.0
    }
}
