use crate::board::{reverse_row, transpose, unpack_col, Board, Row, ROW_MASK};
use std::sync::OnceLock;

const SCORE_LOST_PENALTY: f32 = 200000.0;
const SCORE_MONOTONICITY_POWER: f32 = 4.0;
const SCORE_MONOTONICITY_WEIGHT: f32 = 47.0;
const SCORE_SUM_POWER: f32 = 3.5;
const SCORE_SUM_WEIGHT: f32 = 11.0;
const SCORE_MERGES_WEIGHT: f32 = 700.0;
const SCORE_EMPTY_WEIGHT: f32 = 270.0;

static TABLES: OnceLock<Tables> = OnceLock::new();

pub(crate) struct Tables {
    pub(crate) row_left: Box<[Row; 65536]>,
    pub(crate) row_right: Box<[Row; 65536]>,
    pub(crate) col_up: Box<[Board; 65536]>,
    pub(crate) col_down: Box<[Board; 65536]>,
    pub(crate) heur_score: Box<[f32; 65536]>,
    pub(crate) score: Box<[f32; 65536]>,
}

pub(crate) fn init_tables() {
    let _ = tables();
}

pub(crate) fn tables() -> &'static Tables {
    TABLES.get_or_init(build_tables)
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

pub(crate) fn execute_move(move_id: i32, board: Board) -> Board {
    match move_id {
        0 => execute_move_0(board),
        1 => execute_move_1(board),
        2 => execute_move_2(board),
        3 => execute_move_3(board),
        _ => !0,
    }
}
