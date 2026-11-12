// Syntax-checks every frontend JS file with `node --check`. Covers
// src/js (app code); src/include is vendored (Leaflet, pdfmake, Tauri
// API) and skipped. Also checks that the exports for other audiences
// never mention the contact plan (see PLAN_FREE below). Exit status 1 if
// any check fails.
//
// Usage: node scripts/js-check.mjs
import { readdirSync, readFileSync } from 'node:fs';
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

// The contact plan is printed only on the Contact Plans PDF
// (Visit_Reporting_Design.md §2.3). These files build the exports that go
// to other audiences — the route PDF / Copy Text (map-view.js), the clerk
// PDF, the directory and the data-validation PDFs — and must never read
// it. A source check rather than a rendered one: if none of them names
// the field, none of them can print it.
const PLAN_FREE = [
    'views/map-view.js',
    'views/referrals-pdf.js',
    'views/directory-pdf.js',
    'views/data-validation-pdf.js',
];
const PLAN_WORDS = /contact_plan|contactPlan|CONTACT_PLANS|plan_due|contactPlanLabel/;
let leaks = 0;
for (const f of PLAN_FREE) {
    readFileSync(join(dir, f), 'utf8').split('\n').forEach((line, i) => {
        if (PLAN_WORDS.test(line)) {
            leaks++;
            console.error(`FAIL src/js/${f}:${i + 1} mentions the contact plan, which this export must not print`);
        }
    });
}
console.log(`plan-free exports: ${leaks ? `${leaks} mention(s) found` : `${PLAN_FREE.length} files OK`}`);
process.exit(failed || leaks ? 1 : 0);
