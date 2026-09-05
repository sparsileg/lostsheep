# Vendored Tauri JS API modules

Pulled from npm, not a CDN — same reasoning as `pdfmake` and `leaflet` under
`src/include/`: local-first, no build step, no runtime dependency on any
external host.

- `core.js`, `path.js`, `event.js`, `external/tslib/tslib.es6.js` — from
  `@tauri-apps/api@2.11.1` (matches `tauri-tauri = "2"` in `Cargo.toml`).
- `dialog.js` — from `@tauri-apps/plugin-dialog@2.4.0`'s `dist-js/index.js`
  (matches `tauri-plugin-dialog = "2"` in `Cargo.toml`), with one edit: its
  bare-specifier import (`from '@tauri-apps/api/core'`) was rewritten to the
  relative `from './core.js'`, since there is no bundler or import map here
  to resolve a bare specifier in the browser. No other line was changed.

These replace `window.__TAURI__.*` (the global injected by
`app.withGlobalTauri: true`) now that that setting is `false` — see
`main.rs`/`tauri.conf.json` and issue #17/#18's follow-on.

To upgrade: re-fetch both packages at the versions matching whatever the
Cargo.toml Rust crates are pinned to, and re-apply the same one-line edit to
`dialog.js`.
