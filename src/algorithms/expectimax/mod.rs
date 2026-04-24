use super::{AlgorithmId, MoveResult};
use crate::board::{count_distinct_tiles, count_empty, Board};
use crate::tables::execute_move;
use hashbrown::HashMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) mod heuristic;

use heuristic::score_heur_board;

pub(crate) const CPROB_THRESH_BASE: f32 = 0.0001;
pub(crate) const CACHE_DEPTH_LIMIT: i32 = 15;

const DEFAULT_TRANS_TABLE_CAPACITY: usize = 1 << 20;
const MIN_TRANS_TABLE_CAPACITY: usize = 1 << 12;
const MAX_TRANS_TABLE_CAPACITY: usize = 1 << 20;

static TRANS_TABLE_CAPACITY: AtomicUsize = AtomicUsize::new(DEFAULT_TRANS_TABLE_CAPACITY);

thread_local! {
    static REUSABLE_TRANS_TABLE: RefCell<HashMap<Board, TransEntry>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
pub(crate) struct TransEntry {
    pub(crate) depth: u8,
    pub(crate) heuristic: f32,
}

pub(crate) struct EvalState<'a> {
    trans_table: &'a mut HashMap<Board, TransEntry>,
    pub(crate) curdepth: i32,
    pub(crate) cachehits: u32,
    pub(crate) moves_evaled: u64,
    pub(crate) depth_limit: i32,
}

impl<'a> EvalState<'a> {
    pub(crate) fn new(board: Board, trans_table: &'a mut HashMap<Board, TransEntry>) -> Self {
        trans_table.clear();
        let target_capacity = trans_table_capacity();
        if trans_table.capacity() < target_capacity {
            trans_table.reserve(target_capacity - trans_table.capacity());
        }

        Self {
            trans_table,
            curdepth: 0,
            cachehits: 0,
            moves_evaled: 0,
            depth_limit: 3.max(count_distinct_tiles(board) - 2),
        }
    }
}

pub(crate) fn set_trans_table_capacity(capacity: usize) {
    TRANS_TABLE_CAPACITY.store(
        capacity.clamp(MIN_TRANS_TABLE_CAPACITY, MAX_TRANS_TABLE_CAPACITY),
        Ordering::Relaxed,
    );
}

pub(crate) fn trans_table_capacity() -> usize {
    TRANS_TABLE_CAPACITY.load(Ordering::Relaxed)
}

pub(crate) fn choose_move(board: Board) -> MoveResult {
    REUSABLE_TRANS_TABLE.with(|trans_table| {
        let mut trans_table = trans_table.borrow_mut();
        choose_move_with_table(board, &mut trans_table)
    })
}

pub(crate) fn choose_move_with_table(
    board: Board,
    trans_table: &mut HashMap<Board, TransEntry>,
) -> MoveResult {
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

    MoveResult {
        algorithm: AlgorithmId::Expectimax,
        move_id: best_move,
        depth: state.depth_limit,
        nodes: state.moves_evaled,
        cache_hits: state.cachehits,
    }
}

pub(crate) fn score_toplevel_move(state: &mut EvalState, board: Board, move_id: i32) -> f32 {
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
