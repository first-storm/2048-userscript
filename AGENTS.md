# AGENTS.md

## Project Notes

This repo is a Rust/WASM 2048 AI userscript for `play2048.co`. Keep the existing exported WASM API compatible unless the caller explicitly asks for a breaking change.

## Rust Layout

- `src/lib.rs` is the crate root and re-exports the public WASM-facing functions.
- `src/board.rs` owns the packed board representation, row helpers, transpose, and tile counters.
- `src/tables.rs` owns lookup table initialization and move execution.
- `src/ffi.rs` owns `#[no_mangle]` exports, selected algorithm state, and last-run stats.
- `src/algorithms/mod.rs` owns `AlgorithmId`, `MoveResult`, and algorithm dispatch.
- Each algorithm lives in its own directory module under `src/algorithms/<algorithm>/`.
- `src/algorithms/expectimax/mod.rs` owns the current default algorithm.
- `src/algorithms/expectimax/heuristic.rs` owns expectimax-only board scoring helpers used by the algorithm and WASM score exports.
- `src/algorithms/endgame_tablebase/mod.rs` owns the endgame tablebase algorithm.
- `data/endgame_tablebase.bin` remains at the repo root under `data/`; update `include_bytes!` paths relative to the algorithm module file location.

## Adding A Rust Algorithm

1. Add a new directory module under `src/algorithms/<algorithm>/` with a `mod.rs`.
2. Add a new variant and numeric ID to `AlgorithmId` in `src/algorithms/mod.rs`.
3. Return a `MoveResult` from the algorithm with `move_id`, `depth`, `nodes`, `cache_hits`, and the algorithm ID filled in.
4. Register the module in `choose_move_with_algorithm`.
5. Add the matching developer-facing option to `ALGORITHMS` in `js/userscript.js`.

Algorithm ID `0` is reserved for `Expectimax` and must remain the default.
Algorithm ID `1` is reserved for `EndgameTablebase`.

## JS/WASM Algorithm Selection

- `js/userscript.js` owns the visible Algorithm dropdown.
- The selected algorithm is persisted in `localStorage` using `play2048-wasm-ai.algorithm`.
- Both sync and worker paths must call `set_algorithm` before `choose_move`.
- The JS `ALGORITHMS` list is developer-maintained; it is not a user plugin system.

## Validation

Run these after Rust or userscript changes:

```bash
cargo fmt --check
cargo test
node --check js/userscript.js
node tools/build-userscript.mjs
```

The performance smoke test is intentionally ignored by default.
