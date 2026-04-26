mod algorithms;
mod board;
mod ffi;
mod tables;

pub use ffi::{
    choose_move, current_algorithm, init_tables, last_algorithm, last_cache_hits, last_depth,
    last_nodes, score_board_export, score_greedy_board_export, score_heur_board_export,
    set_algorithm, set_trans_table_capacity,
};

#[cfg(test)]
mod tests {
    use super::algorithms::expectimax::heuristic::score_heur_board;
    use super::algorithms::expectimax::{
        choose_move_with_table, score_toplevel_move, trans_table_capacity, EvalState, TransEntry,
    };
    use super::algorithms::{choose_move_with_algorithm, AlgorithmId};
    use super::board::{count_distinct_tiles, count_empty, reverse_row, transpose, Board};
    use super::ffi::{
        choose_move, current_algorithm, init_tables, last_algorithm, last_cache_hits, last_depth,
        last_nodes, set_algorithm,
    };
    use super::tables::execute_move;
    use hashbrown::HashMap;
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
        if cprob < super::algorithms::expectimax::CPROB_THRESH_BASE
            || state.curdepth >= state.depth_limit
        {
            state.maxdepth = state.maxdepth.max(state.curdepth);
            return score_heur_board(board);
        }

        if state.curdepth < super::algorithms::expectimax::CACHE_DEPTH_LIMIT {
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

        if state.curdepth < super::algorithms::expectimax::CACHE_DEPTH_LIMIT {
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
    fn algorithm_selection_defaults_to_expectimax() {
        assert_eq!(
            set_algorithm(AlgorithmId::Expectimax.as_i32()),
            AlgorithmId::Expectimax.as_i32()
        );
        assert_eq!(current_algorithm(), AlgorithmId::Expectimax.as_i32());
        assert_eq!(set_algorithm(1), AlgorithmId::Greedy.as_i32());
        assert_eq!(current_algorithm(), AlgorithmId::Greedy.as_i32());
        assert_eq!(set_algorithm(2), AlgorithmId::Expectimax.as_i32());
        assert_eq!(current_algorithm(), AlgorithmId::Expectimax.as_i32());
        assert_eq!(set_algorithm(12345), AlgorithmId::Expectimax.as_i32());
        assert_eq!(current_algorithm(), AlgorithmId::Expectimax.as_i32());
    }

    #[test]
    fn dispatch_matches_explicit_expectimax() {
        init_tables();

        for board in sample_boards() {
            let dispatched = choose_move_with_algorithm(AlgorithmId::Expectimax, board);
            let mut table = HashMap::with_capacity(trans_table_capacity());
            let explicit = choose_move_with_table(board, &mut table);

            assert_eq!(dispatched.move_id, explicit.move_id);
            assert_eq!(dispatched.depth, explicit.depth);
            assert_eq!(dispatched.nodes, explicit.nodes);
            assert_eq!(dispatched.cache_hits, explicit.cache_hits);
            assert_eq!(
                dispatched.algorithm.as_i32(),
                AlgorithmId::Expectimax.as_i32()
            );
        }
    }

    #[test]
    fn dispatch_records_greedy_algorithm() {
        init_tables();

        for board in sample_boards() {
            let result = choose_move_with_algorithm(AlgorithmId::Greedy, board);

            assert_eq!(result.algorithm.as_i32(), AlgorithmId::Greedy.as_i32());
            assert_eq!(result.depth, 1);
            assert_eq!(result.cache_hits, 0);
        }
    }

    #[test]
    fn greedy_chooses_legal_move_or_no_move() {
        init_tables();

        for board in sample_boards() {
            let result = choose_move_with_algorithm(AlgorithmId::Greedy, board);

            assert!((0..4).contains(&result.move_id));
            assert_ne!(execute_move(result.move_id, board), board);
            assert!(result.nodes <= 4);
        }

        let locked =
            board_from_ranks([[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12], [13, 14, 15, 1]]);
        let result = choose_move_with_algorithm(AlgorithmId::Greedy, locked);

        assert_eq!(result.move_id, -1);
        assert_eq!(result.nodes, 0);
    }

    #[test]
    fn ffi_choose_move_records_algorithm_and_stats() {
        init_tables();
        set_algorithm(AlgorithmId::Expectimax.as_i32());

        for board in sample_boards() {
            let result = choose_move_with_algorithm(AlgorithmId::Expectimax, board);
            let move_id = choose_move(board);

            assert_eq!(move_id, result.move_id);
            assert_eq!(last_algorithm(), AlgorithmId::Expectimax.as_i32());
            assert_eq!(last_depth(), result.depth);
            assert_eq!(last_nodes(), result.nodes);
            assert_eq!(last_cache_hits(), result.cache_hits);
        }

        set_algorithm(AlgorithmId::Greedy.as_i32());
        for board in sample_boards() {
            let result = choose_move_with_algorithm(AlgorithmId::Greedy, board);
            let move_id = choose_move(board);

            assert_eq!(move_id, result.move_id);
            assert_eq!(last_algorithm(), AlgorithmId::Greedy.as_i32());
            assert_eq!(last_depth(), result.depth);
            assert_eq!(last_nodes(), result.nodes);
            assert_eq!(last_cache_hits(), result.cache_hits);
        }

        set_algorithm(AlgorithmId::Expectimax.as_i32());
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

            let mut expectimax_table = HashMap::with_capacity(trans_table_capacity());
            let expectimax = choose_move_with_table(board, &mut expectimax_table);
            assert_eq!(
                expectimax.move_id,
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
            let fresh = choose_move_with_table(board, &mut fresh_table);

            let reused = choose_move_with_algorithm(AlgorithmId::Expectimax, board);

            assert_eq!(
                fresh.move_id, reused.move_id,
                "move differs for board 0x{board:016x}"
            );
            assert_eq!(
                fresh.nodes, reused.nodes,
                "nodes differ for board 0x{board:016x}"
            );
            assert_eq!(
                fresh.cache_hits, reused.cache_hits,
                "cache hits differ for board 0x{board:016x}"
            );
            assert_eq!(
                fresh.depth, reused.depth,
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
