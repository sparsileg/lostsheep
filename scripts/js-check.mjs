// Syntax-checks every frontend JS file with `node --check`. Covers
// src/js (app code); src/include is vendored (Leaflet, pdfmake, Tauri
// API) and skipped. Exit status 1 if any file fails.
//
// Usage: node scripts/js-check.mjs
import { readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = join(fileURLToPath(import.meta.url), '..', '..');
const dir = join(root, 'src', 'js');

function walk(d) {
    return readdirSync(d, { withFileTypes: true }).flatMap(e =>
        e.isDirectory() ? walk(join(d, e.name)) : e.name.endsWith('.js') ? [join(d, e.name)] : []);
}

const files = walk(dir).sort();
let failed = 0;
for (const f of files) {
    const r = spawnSync(process.execPath, ['--check', f], { encoding: 'utf8' });
    if (r.status !== 0) {
        failed++;
        console.error(`FAIL ${relative(root, f)}\n${r.stderr}`);
    }
}
console.log(`js-check: ${files.length - failed} of ${files.length} files OK`);
process.exit(failed ? 1 : 0);
