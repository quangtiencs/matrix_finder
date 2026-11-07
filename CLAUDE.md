# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Matrix Finder is a cross-platform dual-panel file manager in Rust, built on egui/eframe 0.31 (Rust 1.85+, a C/C++ compiler is needed for unrar and libssh2). It is one binary crate; there is no workspace and no library target.

## Commands

```bash
cargo build                         # debug build
cargo run -- [--new-tab|--new-window] [FOLDER]
cargo test                          # unit tests (~180, a few seconds)
cargo test instance::tests::round_trip   # a single test: module path + name
cargo test -- --ignored <name>      # GUI/integration tests (see below)

make icons                          # regenerate PNG icons, .icns, DMG background from SVGs
make screenshots                    # re-render README screenshots (SHOTS=panels,palette for some)
make app | make dmg | make dmg-universal   # macOS bundle/DMG in target/macos (scripts/macos/build-dmg.sh)
```

- The code is **not** rustfmt-formatted (long lines, no `rustfmt.toml`). Do not run `cargo fmt` over files; match the surrounding style by hand.
- Feature `sftp` (default) pulls in `ssh2`; `vendored-openssl` is used by the DMG script.
- Ignored tests: `src/screenshots.rs` drives the real `App` headlessly with `egui_kittest` + wgpu (keyboard, drag and drop, settings, Quick View stress, `export_screenshots`); they set `HOME` to a demo folder under the temp dir, so they don't touch real settings. `remote.rs` live tests need `MATRIX_FINDER_TEST_FTP` / `MATRIX_FINDER_TEST_SFTP`. `main.rs` has a test-only capped global allocator (`memcap`) used by memory-limit tests.

## Architecture

**`App` (`src/app.rs`, very large)** owns everything: settings (`cfg`), the tab strip, dialogs, toasts, background jobs, remote connections, and the per-frame `update` that polls terminals/jobs/connections and draws the UI. Settings pages live in `settings_ui.rs` as `impl App` methods (it does `use super::*`).

**Tabs, windows, panes:** `tabs::Strip` holds `Workspace`s (tabs); each workspace has a `Vec<Pane>` tiled by a `layout::Layout` split tree (max 2 × 2). The *shown* workspace's panes are **moved out** into `app.panes` / `app.layout` / `app.active` and its slot in the strip is left empty (`app.live` is its uid); `sync_tab()` swaps them back when the active tab changes. So code touching "the current windows" uses `self.panes[self.active]`, while code that must reach every window uses the `all_panes!` macro. After inserting a tab (`insert_workspace`), `sync_tab` makes it current.

**`Pane` → `Tab` (`pane.rs`):** a `Pane` wraps one `Tab`, which is either a file view (`loc: vfs::Location`, entries, cursor, filter, search), a terminal (`term: Option<Box<termview::TermSession>>`), or a special window (`settings`, `transfers`, `du`). Check `is_files()` / `is_terminal()` before assuming. `Tab::navigate` + `reload` take a `ListCtx`, built with the `lc!(self)` macro in `app.rs`.

**Locations (`vfs.rs`):** `Location::{Local, Archive { file, inner }, Remote { id, path }}`. Archives (`archive.rs`) and FTP/SFTP (`remote.rs`, ids into `app.remotes`) are browsed like folders. `App::parse_location_text` turns user text (`~`, `file.zip::inner`, `sftp://…`) into a `Location`.

**Commands and shortcuts:** `shortcuts::Action` is the rebindable key action (per-OS defaults; terminal-safe combos when a terminal has focus); `pane::Cmd` is the command set run by `App::run_cmd`, shared by menus, the command palette (`palette.rs`) and shortcuts. A new command is usually a `Cmd` variant, plus an `Action` if it needs a key.

**Long work runs off the UI thread:** file operations (`ops.rs`) are jobs with progress/pause/cancel shown in the Transfers window; previews, connections, disk usage and checksums use threads + channels polled each frame. Threads wake the UI with `ctx.request_repaint()`.

**Settings (`config.rs`):** `Settings` is `#[serde(default)]` JSON at `<config dir>/matrix_finder/settings.json` (`config::config_dir()`). A new field needs a value in `impl Default for Settings` and loads fine from older files; `normalize()` cleans values after loading. `App::save_cfg` also stores the session (tabs, groups, splits). Enum settings use `#[derive(Default)]` with a `#[default]` variant.

**Single instance (`instance.rs`):** the running app listens on a localhost TCP port. The port and a random token are in `config_dir()/instance.json` (mode 0600). A later launch in `NewTab` mode hands its folder over and exits (`main.rs`). The listener is not started under `cfg(test)`.

**Terminal:** `termview.rs` is the built-in terminal (alacritty_terminal + egui renderer); `terminal.rs` handles terminal profiles, shell detection, `startup_argv`, `sh_quote` and external terminal launching.

**Previews / Quick View:** `preview.rs` dispatches by file type to the viewers (`docview`, `table`, `scidata`, `media`, `rawphoto`, `textdocs`, `fontview`, `model3d`, `certview`); size limits keep huge files from being opened.

## Translations (enforced by tests)

UI strings are English text passed to `i18n::t("…")`; the text itself is the key. Every new `t("…")` string must be added to **both** `KEYS` and the Vietnamese `VI` table in `src/i18n_data.rs`, or `i18n::tests::keys_cover_sources` / `vietnamese_covers_all_keys` fail. (`keys_cover_sources` scans a fixed list of source files in `i18n.rs`; add a new UI module there.)

## Conventions

- Commit messages follow `CONTRIBUTING.md`: `type(scope): summary` (`feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`), imperative, lower case, ≤ 72 chars; breaking changes use `!` and a `BREAKING CHANGE:` footer. Keep i18n entries in the same commit as the feature.
- Releases: tag `v<version>` from `Cargo.toml`, attach the DMG from `make dmg-universal` (ad-hoc signed, not notarized).
- User-visible features are documented in `README.md` (Features list; `README.html`/`README_files/` are rendered output and git-ignored).
