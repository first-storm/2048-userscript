import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const ioFrameworkPath = join(root, "js", "io-framework.js");
const userscriptPath = join(root, "js", "userscript.js");
const wasmPath = join(root, "target", "wasm32-unknown-unknown", "release", "play2048_wasm_ai.wasm");
const distDir = join(root, "dist");
const outputPath = join(distDir, "play2048-wasm-ai.user.js");
const compactOutputPath = join(distDir, "play2048-wasm-ai.min.user.js");

const wasmMarkerStart = "/* WASM_BASE64_START */";
const wasmMarkerEnd = "/* WASM_BASE64_END */";

main();

function main() {
    buildWasm();

    const ioFramework = readRequiredText(ioFrameworkPath);
    const userscript = readRequiredText(userscriptPath);
    const wasmBase64 = readFileSync(wasmPath).toString("base64");

    const header = forceDocumentStart(extractUserscriptHeader(userscript, userscriptPath));
    const ioBody = stripUserscriptHeader(ioFramework, ioFrameworkPath).trim();
    const botBody = embedWasm(stripUserscriptHeader(userscript, userscriptPath).trim(), wasmBase64);
    const bundled = `${header}\n\n${ioBody}\n\n${botBody}\n`;
    const compact = compactUserscript(header, `${ioBody}\n\n${botBody}`);

    validateBundle(bundled);
    validateBundle(compact);
    mkdirSync(distDir, { recursive: true });
    writeFileSync(outputPath, bundled);
    writeFileSync(compactOutputPath, compact);

    const rawBytes = Buffer.byteLength(bundled);
    const compactBytes = Buffer.byteLength(compact);
    console.log(`Built ${relative(outputPath)} (${formatBytes(rawBytes)})`);
    console.log(`Built ${relative(compactOutputPath)} (${formatBytes(compactBytes)})`);
    console.log(`Embedded ${wasmBase64.length} base64 chars from ${relative(wasmPath)}`);
}

function buildWasm() {
    execFileSync("cargo", ["build", "--target", "wasm32-unknown-unknown", "--release"], {
        cwd: root,
        stdio: "inherit",
    });
}

function readRequiredText(path) {
    if (!existsSync(path)) throw new Error(`Missing required file: ${path}`);
    return readFileSync(path, "utf8");
}

function extractUserscriptHeader(source, path) {
    const match = source.match(userscriptHeaderPattern());
    if (!match) throw new Error(`Could not find userscript header in ${path}`);
    return match[0].trimEnd();
}

function forceDocumentStart(header) {
    if (/^\/\/ @run-at\s+/m.test(header)) {
        return header.replace(/^\/\/ @run-at\s+.*$/m, "// @run-at       document-start");
    }
    return header.replace(/^\/\/ ==\/UserScript==$/m, "// @run-at       document-start\n// ==/UserScript==");
}

function stripUserscriptHeader(source, path) {
    const pattern = userscriptHeaderPattern();
    if (!pattern.test(source)) throw new Error(`Could not find userscript header in ${path}`);
    return source.replace(pattern, "").trimStart();
}

function userscriptHeaderPattern() {
    return /^\/\/ ==UserScript==\r?\n[\s\S]*?^\/\/ ==\/UserScript==\r?\n?/m;
}

function embedWasm(source, wasmBase64) {
    const pattern = new RegExp(`${escapeRegExp(wasmMarkerStart)}[\\s\\S]*?${escapeRegExp(wasmMarkerEnd)}`);
    if (!pattern.test(source)) {
        throw new Error(`Could not find ${wasmMarkerStart} / ${wasmMarkerEnd} markers in userscript.js`);
    }
    if (!wasmBase64) throw new Error("WASM payload is empty");
    return source.replace(pattern, `${wasmMarkerStart}\n${JSON.stringify(wasmBase64)}\n${wasmMarkerEnd}`);
}

function validateBundle(source) {
    const headerCount = countMatches(source, /^\/\/ ==UserScript==$/gm);
    if (headerCount !== 1) throw new Error(`Expected exactly one userscript header, found ${headerCount}`);
    for (const symbol of ["window.Play2048IO", "window.Play2048WasmAI"]) {
        if (!source.includes(symbol)) throw new Error(`Bundled userscript is missing ${symbol}`);
    }
    const wasmMatch = source.match(new RegExp(`${escapeRegExp(wasmMarkerStart)}\\s*\\n(".*?")\\s*\\n${escapeRegExp(wasmMarkerEnd)}`, "s"));
    if (!wasmMatch || JSON.parse(wasmMatch[1]).length === 0) {
        throw new Error("Bundled userscript has an empty WASM payload");
    }
}

function compactUserscript(header, body) {
    return `${header}\n${compactJavaScriptBody(body)}\n`;
}

function compactJavaScriptBody(source) {
    return source
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line && !isDisposableComment(line))
        .join("\n");
}

function isDisposableComment(line) {
    return line.startsWith("//") && !line.startsWith("//#");
}

function countMatches(source, pattern) {
    return Array.from(source.matchAll(pattern)).length;
}

function escapeRegExp(value) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function relative(path) {
    return path.startsWith(`${root}/`) ? path.slice(root.length + 1) : path;
}

function formatBytes(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}
