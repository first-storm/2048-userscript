use crate::algorithms::expectimax;
use crate::algorithms::expectimax::heuristic::{score_board, score_heur_board};
use crate::algorithms::{
    choose_move_with_algorithm, normalize_algorithm_id, AlgorithmId, MoveResult,
};
use crate::board::Board;
use crate::tables;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

static CURRENT_ALGORITHM: AtomicI32 = AtomicI32::new(0);
static LAST_ALGORITHM: AtomicI32 = AtomicI32::new(0);
static LAST_NODES: AtomicU64 = AtomicU64::new(0);
static LAST_CACHE_HITS: AtomicU32 = AtomicU32::new(0);
static LAST_DEPTH: AtomicI32 = AtomicI32::new(0);

#[no_mangle]
pub extern "C" fn init_tables() {
    tables::init_tables();
}

#[no_mangle]
pub extern "C" fn set_trans_table_capacity(capacity: usize) {
    expectimax::set_trans_table_capacity(capacity);
}

#[no_mangle]
pub extern "C" fn set_algorithm(id: i32) -> i32 {
    let normalized = normalize_algorithm_id(id);
    CURRENT_ALGORITHM.store(normalized, Ordering::Relaxed);
    normalized
}

#[no_mangle]
pub extern "C" fn current_algorithm() -> i32 {
    normalize_algorithm_id(CURRENT_ALGORITHM.load(Ordering::Relaxed))
}

#[no_mangle]
pub extern "C" fn last_algorithm() -> i32 {
    LAST_ALGORITHM.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn choose_move(board: Board) -> i32 {
    tables::init_tables();
    let algorithm = AlgorithmId::from_i32(current_algorithm());
    let result = choose_move_with_algorithm(algorithm, board);
    record_result(result);
    result.move_id
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
    LAST_NODES.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn last_cache_hits() -> u32 {
    LAST_CACHE_HITS.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn last_depth() -> i32 {
    LAST_DEPTH.load(Ordering::Relaxed)
}

fn record_result(result: MoveResult) {
    LAST_ALGORITHM.store(result.algorithm.as_i32(), Ordering::Relaxed);
    LAST_NODES.store(result.nodes, Ordering::Relaxed);
    LAST_CACHE_HITS.store(result.cache_hits, Ordering::Relaxed);
    LAST_DEPTH.store(result.depth, Ordering::Relaxed);
}
