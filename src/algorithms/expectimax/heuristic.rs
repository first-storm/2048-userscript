use crate::board::{transpose, Board, ROW_MASK};
use crate::tables::tables;

fn score_helper(board: Board, table: &[f32; 65536]) -> f32 {
    table[((board >> 0) & ROW_MASK) as usize]
        + table[((board >> 16) & ROW_MASK) as usize]
        + table[((board >> 32) & ROW_MASK) as usize]
        + table[((board >> 48) & ROW_MASK) as usize]
}

pub(crate) fn score_heur_board(board: Board) -> f32 {
    let tables = tables();
    score_helper(board, &tables.heur_score) + score_helper(transpose(board), &tables.heur_score)
}

pub(crate) fn score_board(board: Board) -> f32 {
    let tables = tables();
    score_helper(board, &tables.score)
}
