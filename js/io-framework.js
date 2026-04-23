// ==UserScript==
// @name         play2048.co Native IO Framework
// @namespace    https://play2048.co/
// @version      0.2.0
// @description  Read play2048.co internal JS state and output moves. No canvas/UI scanning.
// @match        https://play2048.co/*
// @run-at       document-start
// @grant        none
// ==/UserScript==

(() => {
  "use strict";

  const INDEX_RE = /\/assets\/index-[\w-]+\.js(?:\?.*)?$/;
  const native = {
    api: null,
    latest: null,
    unsubscribe: null,
    ready: null,
    resolveReady: null,
    patchedSources: new Set(),
    diagnostics: [],
  };

  native.ready = new Promise((resolve) => {
    native.resolveReady = resolve;
  });

  function patchIndexSource(source) {
    const needle = "}();gi.on(\"move\",na.move),gi.on(\"reset\",na.resetWithConfirm);";
    const injection = [
      "}();",
      "globalThis.__play2048Native=na;",
      "na.subscribe(e=>globalThis.__play2048NativeState=e);",
      "globalThis.dispatchEvent(new CustomEvent('__play2048NativeReady',{detail:na}));",
      "gi.on(\"move\",na.move),gi.on(\"reset\",na.resetWithConfirm);",
    ].join("");

    if (!source.includes(needle)) {
      throw new Error("play2048.co bundle shape changed: native gameplay store not found");
    }
    const assetBase = `${location.origin}/assets/`;
    return source
      .replaceAll('from"./', `from"${assetBase}`)
      .replaceAll('import("./', `import("${assetBase}`)
      .replaceAll('new URL("./', `new URL("${assetBase}`)
      .replace(/new URL\((["'])(?![a-z][a-z\d+.-]*:|\/)(\.\/)?([^"']+)\1\s*,\s*import\.meta\.url\)/gi, (_match, _quote, _dotSlash, path) => {
        return `new URL("${assetBase}${path}")`;
      })
      .replace(needle, injection);
  }

  async function loadPatchedModule(src) {
    const absolute = new URL(src, location.href).href;
    if (native.patchedSources.has(absolute)) return;
    native.patchedSources.add(absolute);
    native.diagnostics.push(`patching ${absolute}`);

    const source = await fetch(absolute, { credentials: "include" }).then((r) => {
      if (!r.ok) throw new Error(`failed to fetch ${absolute}: ${r.status}`);
      return r.text();
    });
    const patched = patchIndexSource(source);
    const blobUrl = URL.createObjectURL(new Blob([patched], { type: "text/javascript" }));
    const script = document.createElement("script");
    script.type = "module";
    script.src = blobUrl;
    script.addEventListener("load", () => URL.revokeObjectURL(blobUrl), { once: true });
    script.addEventListener("error", () => {
      native.diagnostics.push("patched module failed to load");
      console.error("[Play2048IO] patched module failed to load");
    });
    appendWhenPossible(script);
  }

  function appendWhenPossible(node) {
    const parent = document.head || document.documentElement;
    if (parent) {
      parent.appendChild(node);
      return;
    }
    queueMicrotask(() => appendWhenPossible(node));
  }

  function shouldPatchScript(node) {
    return node instanceof HTMLScriptElement
      && node.src
      && INDEX_RE.test(new URL(node.src, location.href).pathname);
  }

  function interceptScriptInsertion() {
    const originalAppendChild = Node.prototype.appendChild;
    const originalInsertBefore = Node.prototype.insertBefore;

    Node.prototype.appendChild = function patchedAppendChild(node) {
      if (shouldPatchScript(node)) {
        loadPatchedModule(node.src).catch((error) => console.error("[Play2048IO]", error));
        return node;
      }
      return originalAppendChild.call(this, node);
    };

    Node.prototype.insertBefore = function patchedInsertBefore(node, child) {
      if (shouldPatchScript(node)) {
        loadPatchedModule(node.src).catch((error) => console.error("[Play2048IO]", error));
        return node;
      }
      return originalInsertBefore.call(this, node, child);
    };

    observeScriptInsertions(new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!shouldPatchScript(node)) continue;
          node.remove();
          loadPatchedModule(node.src).catch((error) => console.error("[Play2048IO]", error));
        }
      }
    }));
  }

  function observeScriptInsertions(observer) {
    if (document.documentElement) {
      observer.observe(document.documentElement, { childList: true, subtree: true });
      return;
    }
    queueMicrotask(() => observeScriptInsertions(observer));
  }

  function patchExistingScripts() {
    for (const node of document.querySelectorAll("script[src]")) {
      if (!shouldPatchScript(node)) continue;
      const src = node.src;
      native.diagnostics.push(`removing original ${src}`);
      node.remove();
      loadPatchedModule(src).catch((error) => {
        native.diagnostics.push(String(error && error.message ? error.message : error));
        console.error("[Play2048IO]", error);
      });
    }
  }

  function installNativeApi(api) {
    if (native.api === api) return;
    native.api = api;
    if (native.unsubscribe) native.unsubscribe();
    native.unsubscribe = api.subscribe((state) => {
      native.latest = state;
    });
    native.resolveReady(api);
  }

  window.addEventListener("__play2048NativeReady", (event) => {
    installNativeApi(event.detail);
  });

  function clonePlain(value, seen = new WeakMap()) {
    if (value === null || typeof value !== "object") return value;
    if (seen.has(value)) return seen.get(value);
    if (typeof value === "function") return undefined;

    if (Array.isArray(value)) {
      const out = [];
      seen.set(value, out);
      for (const item of value) out.push(clonePlain(item, seen));
      return out;
    }

    const out = {};
    seen.set(value, out);
    for (const [key, item] of Object.entries(value)) {
      if (typeof item !== "function") out[key] = clonePlain(item, seen);
    }
    return out;
  }

  function boardToArray(board) {
    const out = Array.from({ length: 4 }, () => Array(4).fill(0));

    if (Array.isArray(board) && Array.isArray(board[0])) {
      for (let r = 0; r < Math.min(4, board.length); r += 1) {
        for (let c = 0; c < Math.min(4, board[r].length); c += 1) {
          out[r][c] = tileValue(board[r][c]);
        }
      }
      return out;
    }

    if (board && typeof board === "object") {
      const rows = board.grid || board.rows || board.matrix;
      if (Array.isArray(rows) && Array.isArray(rows[0])) {
        for (let r = 0; r < Math.min(4, rows.length); r += 1) {
          for (let c = 0; c < Math.min(4, rows[r].length); c += 1) {
            out[r][c] = tileValue(rows[r][c]);
          }
        }
        return out;
      }

      const directTiles = board.tiles || board.cells || board.tileList || board.items;
      if (fillTiles(out, directTiles)) return out;
    }

    const seen = new WeakSet();
    const visit = (value, depth = 0) => {
      if (!value || typeof value !== "object" || seen.has(value) || depth > 8) return;
      seen.add(value);

      const pos = value.position || value.pos;
      if (pos && Number.isInteger(pos.x) && Number.isInteger(pos.y)) {
        const v = tileValue(value);
        if (v && pos.x >= 0 && pos.x < 4 && pos.y >= 0 && pos.y < 4) {
          // play2048.co imports classic cells with { x: row, y: col }.
          out[pos.x][pos.y] = v;
        }
      }

      if (value instanceof Map) {
        for (const item of value.values()) visit(item, depth + 1);
        return;
      }
      if (value instanceof Set) {
        for (const item of value.values()) visit(item, depth + 1);
        return;
      }
      for (const item of Object.values(value)) visit(item, depth + 1);
    };

    visit(board);
    return out;
  }

  function fillTiles(out, tiles) {
    if (!tiles || typeof tiles !== "object") return false;
    let filled = 0;
    const values = tiles instanceof Map || tiles instanceof Set ? tiles.values() : Array.isArray(tiles) ? tiles : Object.values(tiles);
    for (const tile of values) {
      if (!tile || typeof tile !== "object") continue;
      const pos = tile.position || tile.pos;
      if (!pos || !Number.isInteger(pos.x) || !Number.isInteger(pos.y)) continue;
      const v = tileValue(tile);
      if (v && pos.x >= 0 && pos.x < 4 && pos.y >= 0 && pos.y < 4) {
        out[pos.x][pos.y] = v;
        filled += 1;
      }
    }
    return filled > 0;
  }

  function tileValue(tile) {
    if (!tile) return 0;
    if (typeof tile === "number") return tile;
    if (typeof tile.value === "number") return tile.value;
    if (typeof tile.tile === "number") return tile.tile;
    return 0;
  }

  function maxTileInBoard(board) {
    let max = 0;
    for (const row of board) {
      for (const value of row) {
        if (value > max) max = value;
      }
    }
    return max;
  }

  function requireApi() {
    const api = native.api || window.__play2048Native;
    if (!api) throw new Error("native gameplay API not ready yet; await Play2048IO.ready");
    if (!native.api) installNativeApi(api);
    return native.api;
  }

  function readState() {
    requireApi();
    const state = native.latest || window.__play2048NativeState;
    if (!state) throw new Error("native gameplay state not received yet");
    const board = boardToArray(state.board);
    return {
      board,
      score: state.score,
      maxTile: maxTileInBoard(board),
      moveCount: state.moveCount,
      state: state.state,
      over: String(state.state).toLowerCase().includes("over"),
      get powerups() {
        return clonePlain(state.powerups);
      },
      raw: state,
    };
  }

  function move(direction) {
    const key = {
      up: ["ArrowUp", 38],
      right: ["ArrowRight", 39],
      down: ["ArrowDown", 40],
      left: ["ArrowLeft", 37],
    }[direction];
    if (!key) throw new Error(`unknown direction: ${direction}`);
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: key[0],
      code: key[0],
      keyCode: key[1],
      which: key[1],
      bubbles: true,
      cancelable: true,
    }));
  }

  const output = {
    move,
    up: () => move("up"),
    right: () => move("right"),
    down: () => move("down"),
    left: () => move("left"),
    undo: () => requireApi().activatePowerup("Undo"),
    reset: () => requireApi().reset(),
    resetWithConfirm: () => requireApi().resetWithConfirm(),
    activatePowerup: (powerup) => requireApi().activatePowerup(powerup),
    cancelPowerup: (powerup) => requireApi().cancelPowerup(powerup),
  };

  interceptScriptInsertion();
  patchExistingScripts();
  document.addEventListener("readystatechange", patchExistingScripts);

  window.Play2048IO = {
    ready: native.ready,
    readState,
    readBoard: () => readState().board,
    output,
    native,
    diagnostics: native.diagnostics,
  };
})();
