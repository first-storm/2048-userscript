# play2048.co WASM AI Userscript

**这个项目的Vibecoding成分是100%，仅仅作为算法验证**

Rust/WASM 版 2048 AI，用 userscript 注入到 `https://play2048.co/`。JS 部分会先接入网页内部游戏状态，再调用 Rust WASM 的 expectimax bot 输出移动。

## Requirements

- Rust toolchain
- `wasm32-unknown-unknown` target
- Node.js
- Tampermonkey 或其他 userscript 管理器

安装 WASM target：

```bash
rustup target add wasm32-unknown-unknown
```

## Build

在项目根目录运行：

```bash
node tools/build-userscript.mjs
```

构建脚本会自动执行：

```bash
cargo build --target wasm32-unknown-unknown --release
```

然后把 WASM embed 进 userscript，并整合 JS 输入文件。

## Output

构建产物写入 `dist/`，该目录已加入 `.gitignore`。

- `dist/play2048-wasm-ai.min.user.js`
  - 压缩版
  - Tampermonkey 直接加载这个文件
- `dist/play2048-wasm-ai.user.js`
  - 可读整合版
  - 方便调试和检查最终拼接结果

## Source Layout

- `src/lib.rs`
  - Rust/WASM AI 核心
- `js/io-framework.js`
  - 注入 play2048.co 页面 bundle，暴露 `window.Play2048IO`
  - 负责读取棋盘状态和输出移动
- `js/userscript.js`
  - bot UI、WASM 加载、worker fallback、自动运行逻辑
  - 包含 `WASM_BASE64_START` / `WASM_BASE64_END` 标记
- `tools/build-userscript.mjs`
  - 主构建入口
  - 构建 WASM、embed、合并、输出可安装脚本
- `tools/embed-wasm.mjs`
  - 旧的单独 embed 工具，保留作兼容

## Install

1. 运行构建命令。
2. 在 Tampermonkey 中安装或更新：

```text
dist/play2048-wasm-ai.min.user.js
```

3. 打开 `https://play2048.co/`。
4. 页面右下角会出现 `2048 WASM` 控制面板。

## Runtime API

安装后页面上会暴露：

- `window.Play2048IO`
  - `readState()`
  - `readBoard()`
  - `output.move("up" | "down" | "left" | "right")`
- `window.Play2048WasmAI`
  - `start()`
  - `stop()`
  - `step()`
  - `assertWorkerEquivalence()`
  - `boardToBitboard(grid)`

## Notes

- 构建脚本不会覆盖 `js/io-framework.js` 或 `js/userscript.js`。
- Tampermonkey 使用 `.min.user.js`，不是 `.gz`。
- 如果 play2048.co 的前端 bundle 结构变化，`js/io-framework.js` 里的注入匹配可能需要更新。
