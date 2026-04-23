use hashbrown::HashMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

type Board = u64;
type Row = u16;

const ROW_MASK: Board = 0xffff;

const SCORE_LOST_PENALTY: f32 = 200000.0;
const SCORE_MONOTONICITY_POWER: f32 = 4.0;
const SCORE_MONOTONICITY_WEIGHT: f32 = 47.0;
const SCORE_SUM_POWER: f32 = 3.5;
const SCORE_SUM_WEIGHT: f32 = 11.0;
const SCORE_MERGES_WEIGHT: f32 = 700.0;
const SCORE_EMPTY_WEIGHT: f32 = 270.0;

const CPROB_THRESH_BASE: f32 = 0.0001;
const CACHE_DEPTH_LIMIT: i32 = 15;
const DEFAULT_TRANS_TABLE_CAPACITY: usize = 1 << 20;
const MIN_TRANS_TABLE_CAPACITY: usize = 1 << 12;
const MAX_TRANS_TABLE_CAPACITY: usize = 1 << 20;

static TABLES: OnceLock<Tables> = OnceLock::new();
static TRANS_TABLE_CAPACITY: AtomicUsize = AtomicUsize::new(DEFAULT_TRANS_TABLE_CAPACITY);
static mut LAST_NODES: u64 = 0;
static mut LAST_CACHE_HITS: u32 = 0;
static mut LAST_DEPTH: i32 = 0;

thread_local! {
    static REUSABLE_TRANS_TABLE: RefCell<HashMap<Board, TransEntry>> =
        RefCell::new(HashMap::new());
}

struct Tables {
    row_left: Box<[Row; 65536]>,
    row_right: Box<[Row; 65536]>,
    col_up: Box<[Board; 65536]>,
    col_down: Box<[Board; 65536]>,
    heur_score: Box<[f32; 65536]>,
    score: Box<[f32; 65536]>,
}

#[derive(Clone, Copy)]
struct TransEntry {
    depth: u8,
    heuristic: f32,
}

struct EvalState<'a> {
    trans_table: &'a mut HashMap<Board, TransEntry>,
    maxdepth: i32,
    curdepth: i32,
    cachehits: u32,
    moves_evaled: u64,
    depth_limit: i32,
}

impl<'a> EvalState<'a> {
    fn new(board: Board, trans_table: &'a mut HashMap<Board, TransEntry>) -> Self {
        trans_table.clear();
        let target_capacity = trans_table_capacity();
        if trans_table.capacity() < target_capacity {
            trans_table.reserve(target_capacity - trans_table.capacity());
        }

        Self {
            trans_table,
            maxdepth: 0,
            curdepth: 0,
            cachehits: 0,
            moves_evaled: 0,
            depth_limit: 3.max(count_distinct_tiles(board) - 2),
        }
    }
}

#[no_mangle]
pub extern "C" fn init_tables() {
    let _ = tables();
}

#[no_mangle]
pub extern "C" fn set_trans_table_capacity(capacity: usize) {
    TRANS_TABLE_CAPACITY.store(
        capacity.clamp(MIN_TRANS_TABLE_CAPACITY, MAX_TRANS_TABLE_CAPACITY),
        Ordering::Relaxed,
    );
}

#[no_mangle]
pub extern "C" fn choose_move(board: Board) -> i32 {
    let _ = tables();
    REUSABLE_TRANS_TABLE.with(|trans_table| {
        let mut trans_table = trans_table.borrow_mut();
        choose_move_with_table(board, &mut trans_table)
    })
}

fn choose_move_with_table(board: Board, trans_table: &mut HashMap<Board, TransEntry>) -> i32 {
    let mut state = EvalState::new(board, trans_table);
    let mut best = 0.0f32;
    let mut best_move = -1;

    for move_id in 0..4 {
        let res = score_toplevel_move(&mut state, board, move_id);
        if res > best {
            best = res;
            best_move = move_id;
        }
    }

    unsafe {
        LAST_NODES = state.moves_evaled;
        LAST_CACHE_HITS = state.cachehits;
        LAST_DEPTH = state.depth_limit;
    }

    best_move
}

#[no_mangle]
pub extern "C" fn score_heur_board_export(board: Board) -> f64 {
    score_heur_board(board) as f64
}

#[no_mangle]
pub extern "C" fn score_board_export(board: Board) -> f64 {
    score_board(board) as f64
}

#[no_mangle]
pub extern "C" fn last_nodes() -> u64 {
    unsafe { LAST_NODES }
}

#[no_mangle]
pub extern "C" fn last_cache_hits() -> u32 {
    unsafe { LAST_CACHE_HITS }
}

#[no_mangle]
pub extern "C" fn last_depth() -> i32 {
    unsafe { LAST_DEPTH }
}

fn tables() -> &'static Tables {
    TABLES.get_or_init(build_tables)
}

fn trans_table_capacity() -> usize {
    TRANS_TABLE_CAPACITY.load(Ordering::Relaxed)
}

fn reverse_row(row: Row) -> Row {
    ((row >> 12) & 0x000f) | ((row >> 4) & 0x00f0) | ((row << 4) & 0x0f00) | ((row << 12) & 0xf000)
}

fn unpack_col(row: Row) -> Board {
    let row = row as Board;
    (row & 0x000f) | ((row & 0x00f0) << 12) | ((row & 0x0f00) << 24) | ((row & 0xf000) << 36)
}

fn transpose(x: Board) -> Board {
    let a1 = x & 0xF0F00F0FF0F00F0F;
    let a2 = x & 0x0000F0F00000F0F0;
    let a3 = x & 0x0F0F00000F0F0000;
    let a = a1 | (a2 << 12) | (a3 >> 12);
    let b1 = a & 0xFF00FF0000FF00FF;
    let b2 = a & 0x00FF00FF00000000;
    let b3 = a & 0x00000000FF00FF00;
    b1 | (b2 >> 24) | (b3 << 24)
}

fn count_empty(mut x: Board) -> i32 {
    x |= (x >> 2) & 0x3333333333333333;
    x |= x >> 1;
    x = !x & 0x1111111111111111;
    x += x >> 32;
    x += x >> 16;
    x += x >> 8;
    x += x >> 4;
    (x & 0xf) as i32
}

fn count_distinct_tiles(mut board: Board) -> i32 {
    let mut bitset = 0u16;
    while board != 0 {
        bitset |= 1 << (board & 0xf);
        board >>= 4;
    }
    bitset >>= 1;

    let mut count = 0;
    while bitset != 0 {
        bitset &= bitset - 1;
        count += 1;
    }
    count
}

fn build_tables() -> Tables {
    let mut row_left = Box::new([0u16; 65536]);
    let mut row_right = Box::new([0u16; 65536]);
    let mut col_up = Box::new([0u64; 65536]);
    let mut col_down = Box::new([0u64; 65536]);
    let mut heur_score = Box::new([0f32; 65536]);
    let mut score = Box::new([0f32; 65536]);

    for row in 0..65536usize {
        let line = [
            ((row >> 0) & 0xf) as u32,
            ((row >> 4) & 0xf) as u32,
            ((row >> 8) & 0xf) as u32,
            ((row >> 12) & 0xf) as u32,
        ];

        let mut row_score = 0.0f32;
        for rank in line {
            if rank >= 2 {
                row_score += (rank - 1) as f32 * (1u32 << rank) as f32;
            }
        }
        score[row] = row_score;

        let mut sum = 0.0f32;
        let mut empty = 0;
        let mut merges = 0;
        let mut prev = 0;
        let mut counter = 0;

        for rank in line {
            sum += (rank as f32).powf(SCORE_SUM_POWER);
            if rank == 0 {
                empty += 1;
            } else {
                if prev == rank {
                    counter += 1;
                } else if counter > 0 {
                    merges += 1 + counter;
                    counter = 0;
                }
                prev = rank;
            }
        }

        if counter > 0 {
            merges += 1 + counter;
        }

        let mut monotonicity_left = 0.0f32;
        let mut monotonicity_right = 0.0f32;
        for i in 1..4 {
            let left = line[i - 1] as f32;
            let right = line[i] as f32;
            if line[i - 1] > line[i] {
                monotonicity_left +=
                    left.powf(SCORE_MONOTONICITY_POWER) - right.powf(SCORE_MONOTONICITY_POWER);
            } else {
                monotonicity_right +=
                    right.powf(SCORE_MONOTONICITY_POWER) - left.powf(SCORE_MONOTONICITY_POWER);
            }
        }

        heur_score[row] = SCORE_LOST_PENALTY
            + SCORE_EMPTY_WEIGHT * empty as f32
            + SCORE_MERGES_WEIGHT * merges as f32
            - SCORE_MONOTONICITY_WEIGHT * monotonicity_left.min(monotonicity_right)
            - SCORE_SUM_WEIGHT * sum;

        // The C++ loop retries the same position after sliding into an empty slot.
        let mut line = [
            ((row >> 0) & 0xf) as u32,
            ((row >> 4) & 0xf) as u32,
            ((row >> 8) & 0xf) as u32,
            ((row >> 12) & 0xf) as u32,
        ];
        let mut i = 0isize;
        while i < 3 {
            let idx = i as usize;
            let mut j = idx + 1;
            while j < 4 && line[j] == 0 {
                j += 1;
            }
            if j == 4 {
                break;
            }

            if line[idx] == 0 {
                line[idx] = line[j];
                line[j] = 0;
                i -= 1;
            } else if line[idx] == line[j] {
                if line[idx] != 0xf {
                    line[idx] += 1;
                }
                line[j] = 0;
            }
            i += 1;
        }

        let result = ((line[0] << 0) | (line[1] << 4) | (line[2] << 8) | (line[3] << 12)) as Row;
        let rev_result = reverse_row(result);
        let rev_row = reverse_row(row as Row) as usize;

        row_left[row] = row as Row ^ result;
        row_right[rev_row] = rev_row as Row ^ rev_result;
        col_up[row] = unpack_col(row as Row) ^ unpack_col(result);
        col_down[rev_row] = unpack_col(rev_row as Row) ^ unpack_col(rev_result);
    }

    Tables {
        row_left,
        row_right,
        col_up,
        col_down,
        heur_score,
        score,
    }
}

fn execute_move_0(board: Board) -> Board {
    let tables = tables();
    let mut ret = board;
    let t = transpose(board);
    ret ^= tables.col_up[((t >> 0) & ROW_MASK) as usize] << 0;
    ret ^= tables.col_up[((t >> 16) & ROW_MASK) as usize] << 4;
    ret ^= tables.col_up[((t >> 32) & ROW_MASK) as usize] << 8;
    ret ^= tables.col_up[((t >> 48) & ROW_MASK) as usize] << 12;
    ret
}

fn execute_move_1(board: Board) -> Board {
    let tables = tables();
    let mut ret = board;
    let t = transpose(board);
    ret ^= tables.col_down[((t >> 0) & ROW_MASK) as usize] << 0;
    ret ^= tables.col_down[((t >> 16) & ROW_MASK) as usize] << 4;
    ret ^= tables.col_down[((t >> 32) & ROW_MASK) as usize] << 8;
    ret ^= tables.col_down[((t >> 48) & ROW_MASK) as usize] << 12;
    ret
}

fn execute_move_2(board: Board) -> Board {
    let tables = tables();
    let mut ret = board;
    ret ^= (tables.row_left[((board >> 0) & ROW_MASK) as usize] as Board) << 0;
    ret ^= (tables.row_left[((board >> 16) & ROW_MASK) as usize] as Board) << 16;
    ret ^= (tables.row_left[((board >> 32) & ROW_MASK) as usize] as Board) << 32;
    ret ^= (tables.row_left[((board >> 48) & ROW_MASK) as usize] as Board) << 48;
    ret
}

fn execute_move_3(board: Board) -> Board {
    let tables = tables();
    let mut ret = board;
    ret ^= (tables.row_right[((board >> 0) & ROW_MASK) as usize] as Board) << 0;
    ret ^= (tables.row_right[((board >> 16) & ROW_MASK) as usize] as Board) << 16;
    ret ^= (tables.row_right[((board >> 32) & ROW_MASK) as usize] as Board) << 32;
    ret ^= (tables.row_right[((board >> 48) & ROW_MASK) as usize] as Board) << 48;
    ret
}

fn execute_move(move_id: i32, board: Board) -> Board {
    match move_id {
        0 => execute_move_0(board),
        1 => execute_move_1(board),
        2 => execute_move_2(board),
        3 => execute_move_3(board),
        _ => !0,
    }
}

fn score_helper(board: Board, table: &[f32; 65536]) -> f32 {
    table[((board >> 0) & ROW_MASK) as usize]
        + table[((board >> 16) & ROW_MASK) as usize]
        + table[((board >> 32) & ROW_MASK) as usize]
        + table[((board >> 48) & ROW_MASK) as usize]
}

fn score_heur_board(board: Board) -> f32 {
    let tables = tables();
    score_helper(board, &tables.heur_score) + score_helper(transpose(board), &tables.heur_score)
}

fn score_board(board: Board) -> f32 {
    let tables = tables();
    score_helper(board, &tables.score)
}

fn score_toplevel_move(state: &mut EvalState, board: Board, move_id: i32) -> f32 {
    let new_board = execute_move(move_id, board);
    if board == new_board {
        return 0.0;
    }
    score_tilechoose_node(state, new_board, 1.0) + 1e-6
}

fn score_move_node(state: &mut EvalState, board: Board, cprob: f32) -> f32 {
    let mut best = 0.0f32;
    state.curdepth += 1;

    for move_id in 0..4 {
        let new_board = execute_move(move_id, board);
        state.moves_evaled += 1;
        if board != new_board {
            best = best.max(score_tilechoose_node(state, new_board, cprob));
        }
    }

    state.curdepth -= 1;
    best
}

fn score_tilechoose_node(state: &mut EvalState, board: Board, cprob: f32) -> f32 {
    if cprob < CPROB_THRESH_BASE || state.curdepth >= state.depth_limit {
        state.maxdepth = state.maxdepth.max(state.curdepth);
        return score_heur_board(board);
    }

    if state.curdepth < CACHE_DEPTH_LIMIT {
        if let Some(entry) = state.trans_table.get(&board) {
            if entry.depth as i32 <= state.curdepth {
                state.cachehits += 1;
                return entry.heuristic;
            }
        }
    }

    let num_open = count_empty(board);
    if num_open == 0 {
        return score_move_node(state, board, cprob);
    }

    let next_prob = cprob / num_open as f32;
    let mut res = 0.0f32;
    let mut tmp = board;
    let mut tile_2 = 1u64;

    for _ in 0..16 {
        if tmp & 0xf == 0 {
            res += score_move_node(state, board | tile_2, next_prob * 0.9) * 0.9;
            res += score_move_node(state, board | (tile_2 << 1), next_prob * 0.1) * 0.1;
        }
        tmp >>= 4;
        tile_2 <<= 4;
    }

    res /= num_open as f32;

    if state.curdepth < CACHE_DEPTH_LIMIT {
        state.trans_table.insert(
            board,
            TransEntry {
                depth: state.curdepth as u8,
                heuristic: res,
            },
        );
    }

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;
    use std::time::Instant;

    fn board_from_ranks(ranks: [[u64; 4]; 4]) -> Board {
        let mut board = 0;
        let mut shift = 0;
        for row in ranks {
            for rank in row {
                board |= rank << shift;
                shift += 4;
            }
        }
        board
    }

    struct StdEvalState {
        trans_table: StdHashMap<Board, TransEntry>,
        maxdepth: i32,
        curdepth: i32,
        cachehits: u32,
        moves_evaled: u64,
        depth_limit: i32,
    }

    impl StdEvalState {
        fn new(board: Board) -> Self {
            Self {
                trans_table: StdHashMap::with_capacity(trans_table_capacity()),
                maxdepth: 0,
                curdepth: 0,
                cachehits: 0,
                moves_evaled: 0,
                depth_limit: 3.max(count_distinct_tiles(board) - 2),
            }
        }
    }

    fn score_toplevel_move_std(state: &mut StdEvalState, board: Board, move_id: i32) -> f32 {
        let new_board = execute_move(move_id, board);
        if board == new_board {
            return 0.0;
        }
        score_tilechoose_node_std(state, new_board, 1.0) + 1e-6
    }

    fn score_move_node_std(state: &mut StdEvalState, board: Board, cprob: f32) -> f32 {
        let mut best = 0.0f32;
        state.curdepth += 1;

        for move_id in 0..4 {
            let new_board = execute_move(move_id, board);
            state.moves_evaled += 1;
            if board != new_board {
                best = best.max(score_tilechoose_node_std(state, new_board, cprob));
            }
        }

        state.curdepth -= 1;
        best
    }

    fn score_tilechoose_node_std(state: &mut StdEvalState, board: Board, cprob: f32) -> f32 {
        if cprob < CPROB_THRESH_BASE || state.curdepth >= state.depth_limit {
            state.maxdepth = state.maxdepth.max(state.curdepth);
            return score_heur_board(board);
        }

        if state.curdepth < CACHE_DEPTH_LIMIT {
            if let Some(entry) = state.trans_table.get(&board) {
                if entry.depth as i32 <= state.curdepth {
                    state.cachehits += 1;
                    return entry.heuristic;
                }
            }
        }

        let num_open = count_empty(board);
        if num_open == 0 {
            return score_move_node_std(state, board, cprob);
        }

        let next_prob = cprob / num_open as f32;
        let mut res = 0.0f32;
        let mut tmp = board;
        let mut tile_2 = 1u64;

        for _ in 0..16 {
            if tmp & 0xf == 0 {
                res += score_move_node_std(state, board | tile_2, next_prob * 0.9) * 0.9;
                res += score_move_node_std(state, board | (tile_2 << 1), next_prob * 0.1) * 0.1;
            }
            tmp >>= 4;
            tile_2 <<= 4;
        }

        res /= num_open as f32;

        if state.curdepth < CACHE_DEPTH_LIMIT {
            state.trans_table.insert(
                board,
                TransEntry {
                    depth: state.curdepth as u8,
                    heuristic: res,
                },
            );
        }

        res
    }

    fn choose_move_std(board: Board) -> i32 {
        let mut state = StdEvalState::new(board);
        let mut best = 0.0f32;
        let mut best_move = -1;

        for move_id in 0..4 {
            let res = score_toplevel_move_std(&mut state, board, move_id);
            if res > best {
                best = res;
                best_move = move_id;
            }
        }

        best_move
    }

    fn sample_boards() -> [Board; 6] {
        [
            board_from_ranks([[1, 2, 3, 4], [0, 1, 0, 2], [0, 0, 1, 0], [0, 0, 0, 0]]),
            board_from_ranks([[4, 3, 2, 1], [3, 2, 1, 0], [2, 1, 0, 0], [1, 0, 0, 0]]),
            board_from_ranks([[8, 7, 6, 5], [7, 6, 5, 4], [6, 5, 4, 3], [0, 0, 1, 2]]),
            board_from_ranks([[11, 10, 9, 8], [7, 6, 5, 4], [3, 2, 1, 0], [0, 0, 0, 0]]),
            board_from_ranks([[1, 1, 2, 2], [3, 3, 4, 4], [5, 5, 6, 6], [7, 0, 0, 0]]),
            board_from_ranks([[12, 11, 10, 9], [8, 7, 6, 5], [4, 3, 2, 1], [0, 0, 0, 0]]),
        ]
    }

    #[test]
    fn reverses_rows() {
        assert_eq!(reverse_row(0x1234), 0x4321);
        assert_eq!(reverse_row(0x0f10), 0x01f0);
    }

    #[test]
    fn transposes_board() {
        let board = 0xfedc_ba98_7654_3210;
        assert_eq!(transpose(transpose(board)), board);
    }

    #[test]
    fn counts_empty_tiles() {
        assert_eq!(count_empty(0x1111_1111_1111_1111), 0);
        assert_eq!(count_empty(0x0000_0000_0000_0001), 15);
    }

    #[test]
    fn executes_left_and_right_moves() {
        init_tables();
        let board = board_from_ranks([[1, 0, 1, 2], [0, 0, 2, 2], [3, 0, 0, 3], [1, 2, 3, 4]]);
        let left = execute_move(2, board);
        let right = execute_move(3, board);

        assert_eq!(
            left,
            board_from_ranks([[2, 2, 0, 0], [3, 0, 0, 0], [4, 0, 0, 0], [1, 2, 3, 4]])
        );
        assert_eq!(
            right,
            board_from_ranks([[0, 0, 2, 2], [0, 0, 0, 3], [0, 0, 0, 4], [1, 2, 3, 4]])
        );
    }

    #[test]
    fn executes_up_and_down_moves() {
        init_tables();
        let board = board_from_ranks([[1, 0, 0, 1], [1, 2, 0, 0], [0, 2, 3, 1], [0, 0, 3, 1]]);

        assert_eq!(
            execute_move(0, board),
            board_from_ranks([[2, 3, 4, 2], [0, 0, 0, 1], [0, 0, 0, 0], [0, 0, 0, 0]])
        );
        assert_eq!(
            execute_move(1, board),
            board_from_ranks([[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [2, 3, 4, 2]])
        );
    }

    #[test]
    fn chooses_legal_move() {
        init_tables();
        let board = board_from_ranks([[1, 2, 3, 4], [0, 1, 0, 2], [0, 0, 1, 0], [0, 0, 0, 0]]);
        let move_id = choose_move(board);
        assert!((0..4).contains(&move_id));
        assert_ne!(execute_move(move_id, board), board);
    }

    #[test]
    fn hashbrown_cache_matches_std_hashmap_reference() {
        init_tables();

        for board in sample_boards() {
            let mut fast_table = HashMap::with_capacity(trans_table_capacity());
            let mut fast_state = EvalState::new(board, &mut fast_table);
            let mut ref_state = StdEvalState::new(board);

            for move_id in 0..4 {
                let fast = score_toplevel_move(&mut fast_state, board, move_id);
                let reference = score_toplevel_move_std(&mut ref_state, board, move_id);
                assert_eq!(
                    fast.to_bits(),
                    reference.to_bits(),
                    "move {move_id} differs for board 0x{board:016x}"
                );
            }

            assert_eq!(
                choose_move(board),
                choose_move_std(board),
                "best move differs for board 0x{board:016x}"
            );
        }
    }

    #[test]
    fn reusable_trans_table_matches_fresh_hashbrown() {
        init_tables();

        for board in sample_boards() {
            let mut fresh_table = HashMap::with_capacity(trans_table_capacity());
            let fresh_move = choose_move_with_table(board, &mut fresh_table);
            let fresh_nodes = unsafe { LAST_NODES };
            let fresh_cache_hits = unsafe { LAST_CACHE_HITS };
            let fresh_depth = unsafe { LAST_DEPTH };

            let reused_move = choose_move(board);
            let reused_nodes = unsafe { LAST_NODES };
            let reused_cache_hits = unsafe { LAST_CACHE_HITS };
            let reused_depth = unsafe { LAST_DEPTH };

            assert_eq!(
                fresh_move, reused_move,
                "move differs for board 0x{board:016x}"
            );
            assert_eq!(
                fresh_nodes, reused_nodes,
                "nodes differ for board 0x{board:016x}"
            );
            assert_eq!(
                fresh_cache_hits, reused_cache_hits,
                "cache hits differ for board 0x{board:016x}"
            );
            assert_eq!(
                fresh_depth, reused_depth,
                "depth differs for board 0x{board:016x}"
            );
        }
    }

    #[test]
    #[ignore = "performance smoke test; run with --ignored --nocapture"]
    fn hashbrown_performance_smoke() {
        init_tables();
        let boards = sample_boards();

        let start = Instant::now();
        let mut fast_sum = 0i32;
        for _ in 0..3 {
            for board in boards {
                fast_sum += choose_move(board);
            }
        }
        let fast_elapsed = start.elapsed();

        let start = Instant::now();
        let mut ref_sum = 0i32;
        for _ in 0..3 {
            for board in boards {
                ref_sum += choose_move_std(board);
            }
        }
        let ref_elapsed = start.elapsed();

        assert_eq!(fast_sum, ref_sum);
        println!("hashbrown: {fast_elapsed:?}, std HashMap reference: {ref_elapsed:?}");
    }
}
