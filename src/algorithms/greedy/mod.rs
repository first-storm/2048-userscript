use super::{AlgorithmId, MoveResult};
use crate::algorithms::expectimax::heuristic::score_board;
use crate::board::{count_empty, Board};
use crate::tables::execute_move;

const EMPTY_WEIGHT: f32 = 1200.0;
const MERGE_WEIGHT: f32 = 850.0;
const ROUGHNESS_WEIGHT: f32 = 18.0;
const CORNER_MAX_WEIGHT: f32 = 2500.0;
const SNAKE_WEIGHT_SCALE: f32 = 2.5;

const SNAKES: [[usize; 16]; 4] = [
    [0, 1, 2, 3, 7, 6, 5, 4, 8, 9, 10, 11, 15, 14, 13, 12],
    [3, 2, 1, 0, 4, 5, 6, 7, 11, 10, 9, 8, 12, 13, 14, 15],
    [12, 13, 14, 15, 11, 10, 9, 8, 4, 5, 6, 7, 3, 2, 1, 0],
    [15, 14, 13, 12, 8, 9, 10, 11, 7, 6, 5, 4, 0, 1, 2, 3],
];

const CORNERS: [usize; 4] = [0, 3, 12, 15];

#[derive(Clone, Copy, Debug)]
struct Candidate {
    move_id: i32,
    heuristic: f32,
    merge_gain: f32,
}

pub(crate) fn choose_move(board: Board) -> MoveResult {
    let base_score = score_board(board);
    let mut best = None;
    let mut nodes = 0;

    for move_id in 0..4 {
        let new_board = execute_move(move_id, board);
        if new_board == board {
            continue;
        }

        nodes += 1;
        let candidate = Candidate {
            move_id,
            heuristic: score_greedy_board(new_board),
            merge_gain: score_board(new_board) - base_score,
        };

        if best.is_none_or(|best| is_better(candidate, best)) {
            best = Some(candidate);
        }
    }

    MoveResult {
        algorithm: AlgorithmId::Greedy,
        move_id: best.map_or(-1, |candidate| candidate.move_id),
        depth: 1,
        nodes,
        cache_hits: 0,
    }
}

pub(crate) fn score_greedy_board(board: Board) -> f32 {
    let ranks = ranks(board);
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let empty = count_empty(board) as f32;

    best_snake_score(&ranks) + EMPTY_WEIGHT * empty + MERGE_WEIGHT * merge_opportunities(&ranks)
        - ROUGHNESS_WEIGHT * roughness(&ranks)
        + corner_max_bonus(&ranks, max_rank)
}

fn is_better(candidate: Candidate, best: Candidate) -> bool {
    candidate.heuristic > best.heuristic
        || (candidate.heuristic == best.heuristic && candidate.merge_gain > best.merge_gain)
}

fn ranks(mut board: Board) -> [u8; 16] {
    let mut ranks = [0; 16];
    for rank in &mut ranks {
        *rank = (board & 0xf) as u8;
        board >>= 4;
    }
    ranks
}

fn best_snake_score(ranks: &[u8; 16]) -> f32 {
    let mut best = f32::NEG_INFINITY;

    for snake in SNAKES {
        let mut weight = 1 << 15;
        let mut score = 0.0;
        for idx in snake {
            score += ranks[idx] as f32 * weight as f32;
            weight >>= 1;
        }
        best = best.max(score);
    }

    best * SNAKE_WEIGHT_SCALE
}

fn merge_opportunities(ranks: &[u8; 16]) -> f32 {
    let mut merges = 0;
    for y in 0..4 {
        for x in 0..4 {
            let rank = ranks[y * 4 + x];
            if rank == 0 {
                continue;
            }
            if x < 3 && ranks[y * 4 + x + 1] == rank {
                merges += 1;
            }
            if y < 3 && ranks[(y + 1) * 4 + x] == rank {
                merges += 1;
            }
        }
    }
    merges as f32
}

fn roughness(ranks: &[u8; 16]) -> f32 {
    let mut roughness = 0;
    for y in 0..4 {
        for x in 0..4 {
            let rank = ranks[y * 4 + x];
            if rank == 0 {
                continue;
            }
            if x < 3 {
                roughness += rank.abs_diff(ranks[y * 4 + x + 1]) as u32;
            }
            if y < 3 {
                roughness += rank.abs_diff(ranks[(y + 1) * 4 + x]) as u32;
            }
        }
    }
    roughness as f32
}

fn corner_max_bonus(ranks: &[u8; 16], max_rank: u8) -> f32 {
    if max_rank == 0 || !CORNERS.iter().any(|&idx| ranks[idx] == max_rank) {
        return 0.0;
    }
    CORNER_MAX_WEIGHT * max_rank as f32
}

#[cfg(test)]
mod tests {
    use super::{is_better, Candidate};

    #[test]
    fn tie_break_prefers_higher_merge_gain() {
        let candidate = Candidate {
            move_id: 1,
            heuristic: 42.0,
            merge_gain: 8.0,
        };
        let best = Candidate {
            move_id: 0,
            heuristic: 42.0,
            merge_gain: 4.0,
        };

        assert!(is_better(candidate, best));
    }

    #[test]
    fn final_tie_keeps_existing_direction_order() {
        let candidate = Candidate {
            move_id: 1,
            heuristic: 42.0,
            merge_gain: 4.0,
        };
        let best = Candidate {
            move_id: 0,
            heuristic: 42.0,
            merge_gain: 4.0,
        };

        assert!(!is_better(candidate, best));
    }
}
