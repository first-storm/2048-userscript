use crate::board::Board;

pub(crate) mod expectimax;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlgorithmId {
    Expectimax,
}

impl AlgorithmId {
    pub(crate) fn from_i32(id: i32) -> Self {
        match id {
            0 => Self::Expectimax,
            _ => Self::Expectimax,
        }
    }

    pub(crate) fn as_i32(self) -> i32 {
        match self {
            Self::Expectimax => 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MoveResult {
    pub(crate) algorithm: AlgorithmId,
    pub(crate) move_id: i32,
    pub(crate) depth: i32,
    pub(crate) nodes: u64,
    pub(crate) cache_hits: u32,
}

pub(crate) fn normalize_algorithm_id(id: i32) -> i32 {
    AlgorithmId::from_i32(id).as_i32()
}

pub(crate) fn choose_move_with_algorithm(algorithm: AlgorithmId, board: Board) -> MoveResult {
    match algorithm {
        AlgorithmId::Expectimax => expectimax::choose_move(board),
    }
}
