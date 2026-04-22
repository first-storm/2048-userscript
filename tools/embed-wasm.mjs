import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const wasmPath = join(root, "target", "wasm32-unknown-unknown", "release", "play2048_wasm_ai.wasm");
const userscriptPath = join(root, "js", "userscript.js");

const wasmBase64 = readFileSync(wasmPath).toString("base64");
const userscript = readFileSync(userscriptPath, "utf8");

const start = "/* WASM_BASE64_START */";
const end = "/* WASM_BASE64_END */";
const pattern = new RegExp(`${escapeRegExp(start)}[\\s\\S]*?${escapeRegExp(end)}`);

if (!pattern.test(userscript)) {
    throw new Error(`Could not find ${start} / ${end} markers in userscript.js`);
}

const next = userscript.replace(pattern, `${start}\n${JSON.stringify(wasmBase64)}\n${end}`);
writeFileSync(userscriptPath, next);
console.log(`Embedded ${wasmBase64.length} base64 chars from ${wasmPath}`);

function escapeRegExp(value) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
