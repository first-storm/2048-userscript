import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const defaultInput = join(root, "vendor", "2048EndgameTablebase", "ai_and_sort", "src");
const inputDir = process.argv[2] || defaultInput;
const outputPath = process.argv[3] || join(root, "data", "endgame_tablebase.bin");
const defaultArchive = join(root, "vendor", "2048EndgameTablebase", "ai_and_sort", "egtb_data.7z");
const archiveUrl =
    "https://raw.githubusercontent.com/game-difficulty/2048EndgameTablebase/main/ai_and_sort/egtb_data.7z";

const tables = [
    { type: 256, layers: 72, initialSum: 8, lower: 0.75, upper: 0.99999, file: "egtb_data_256.cpp" },
    { type: 512, layers: 248, initialSum: 8, lower: 0.4, upper: 0.96, file: "egtb_data_512.cpp" },
    { type: 1256, layers: 72, initialSum: 18, lower: 0.05, upper: 0.28, flags: 1, file: "egtb_data_1256.cpp" },
];

await ensureInputFiles();

const chunks = [];
const tableRecords = [];
let blobOffset = 0;

for (const table of tables) {
    const source = readFileSync(join(inputDir, table.file), "utf8");
    const prefix = `EGTB${table.type}`;
    const b = parseTopArray(source, `${prefix}_B`);
    const l = parseTopArray(source, `${prefix}_L`);
    if (b.length !== table.layers || l.length !== table.layers) {
        throw new Error(`${table.file}: expected ${table.layers} B/L entries, got ${b.length}/${l.length}`);
    }

    const layers = [];
    for (let layer = 0; layer < table.layers; layer++) {
        const seeds = b[layer] ? parseLayerArray(source, layer, "seeds") : [];
        const sigs = l[layer] ? parseLayerArray(source, layer, "sigs") : [];
        const rates = l[layer] ? parseLayerArray(source, layer, "rates") : [];
        if (seeds.length !== b[layer] || sigs.length !== l[layer] || rates.length !== l[layer]) {
            throw new Error(`${table.file}: layer ${layer} length mismatch`);
        }

        const seedBytes = u16Bytes(seeds);
        const sigBytes = u8Bytes(sigs);
        const rateBytes = u16Bytes(rates);
        const seedOffset = blobOffset; chunks.push(seedBytes); blobOffset += seedBytes.length;
        const sigOffset = blobOffset; chunks.push(sigBytes); blobOffset += sigBytes.length;
        const rateOffset = blobOffset; chunks.push(rateBytes); blobOffset += rateBytes.length;

        layers.push({
            b: b[layer],
            l: l[layer],
            seedOffset,
            seedLen: seeds.length,
            sigOffset,
            sigLen: sigs.length,
            rateOffset,
            rateLen: rates.length,
        });
    }
    tableRecords.push({ ...table, flags: table.flags || 0, layers });
}

const headerBytes = 8 + 4;
const tableBytes = tableRecords.reduce((sum, table) => sum + 24 + table.layers.length * 32, 0);
const out = Buffer.alloc(headerBytes + tableBytes + blobOffset);
let pos = 0;
out.write("P8ETB001", pos, "ascii"); pos += 8;
pos = writeU32(out, pos, tableRecords.length);

for (const table of tableRecords) {
    pos = writeU32(out, pos, table.type);
    pos = writeU32(out, pos, table.layers.length);
    pos = writeI32(out, pos, table.initialSum);
    pos = writeF32(out, pos, table.lower);
    pos = writeF32(out, pos, table.upper);
    pos = writeU32(out, pos, table.flags);
    for (const layer of table.layers) {
        pos = writeU32(out, pos, layer.b);
        pos = writeU32(out, pos, layer.l);
        pos = writeU32(out, pos, layer.seedOffset);
        pos = writeU32(out, pos, layer.seedLen);
        pos = writeU32(out, pos, layer.sigOffset);
        pos = writeU32(out, pos, layer.sigLen);
        pos = writeU32(out, pos, layer.rateOffset);
        pos = writeU32(out, pos, layer.rateLen);
    }
}

for (const chunk of chunks) {
    chunk.copy(out, pos);
    pos += chunk.length;
}

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, out);
console.log(`Built ${basename(outputPath)}: ${out.length} bytes from ${inputDir}`);

async function ensureInputFiles() {
    if (tables.every((table) => existsSync(join(inputDir, table.file)))) {
        return;
    }
    if (inputDir !== defaultInput) {
        throw new Error(`Missing tablebase sources in ${inputDir}`);
    }

    mkdirSync(dirname(defaultArchive), { recursive: true });
    mkdirSync(defaultInput, { recursive: true });

    if (!existsSync(defaultArchive)) {
        console.log(`Downloading ${archiveUrl}`);
        const response = await fetch(archiveUrl);
        if (!response.ok) {
            throw new Error(`failed to download tablebase archive: ${response.status} ${response.statusText}`);
        }
        writeFileSync(defaultArchive, Buffer.from(await response.arrayBuffer()));
    }

    console.log(`Extracting ${basename(defaultArchive)} to ${inputDir}`);
    execFileSync("7z", ["x", defaultArchive, `-o${inputDir}`, "-y"], { stdio: "inherit" });
}

function parseTopArray(source, name) {
    const match = source.match(new RegExp(`const\\s+uint32_t\\s+${name}\\s*\\[[^\\]]+\\]\\s*=\\s*\\{([\\s\\S]*?)\\};`));
    if (!match) throw new Error(`Missing ${name}`);
    return parseNumbers(match[1]);
}

function parseLayerArray(source, layer, suffix) {
    const type = suffix === "sigs" ? "uint8_t" : "uint16_t";
    const pattern = `static\\s+const\\s+${type}\\s+layer_${layer}_${suffix}\\s*\\[\\]\\s*=\\s*\\{([\\s\\S]*?)\\};`;
    const match = source.match(new RegExp(pattern));
    if (!match) throw new Error(`Missing layer_${layer}_${suffix}`);
    return parseNumbers(match[1]);
}

function parseNumbers(body) {
    return Array.from(body.matchAll(/0x[0-9a-fA-F]+|\d+/g), (m) => Number(m[0]));
}

function u8Bytes(values) {
    return Buffer.from(values.map((v) => v & 0xff));
}

function u16Bytes(values) {
    const buf = Buffer.alloc(values.length * 2);
    values.forEach((v, i) => buf.writeUInt16LE(v & 0xffff, i * 2));
    return buf;
}

function writeU32(buf, pos, value) {
    buf.writeUInt32LE(value >>> 0, pos);
    return pos + 4;
}

function writeI32(buf, pos, value) {
    buf.writeInt32LE(value | 0, pos);
    return pos + 4;
}

function writeF32(buf, pos, value) {
    buf.writeFloatLE(value, pos);
    return pos + 4;
}
