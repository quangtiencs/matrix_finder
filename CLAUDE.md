# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Matrix Finder is a cross-platform dual-panel file manager in Rust, built on egui/eframe 0.31 (Rust 1.85+, a C/C++ compiler is needed for unrar and libssh2). It is one binary crate; there is no workspace and no library target.

## Principles (non-negotiable)

These come before features, convenience and speed. A change that breaks one of them is a bug, however useful it is.

1. **Users' files are never damaged.**
   - Browsing and Quick View are strictly read-only. A preview never writes, renames, locks, touches or "repairs" the file it shows, nor creates anything next to it. Open files read-only (e.g. SQLite with `SQLITE_OPEN_READ_ONLY` and `immutable`), and do the same for external tools such as FFmpeg, ImageMagick and git.
   - Code that changes files (copy, move, delete, rename, batch rename, extract, compress, upload/download, settings, backup restore) needs a review of three things before it ships:
     - **logic**: conflicts, overwrites, symlinks, case-insensitive disks, a target inside its source;
     - **UI**: what the user is shown and asked before anything is lost (no stray Enter or click can confirm a destructive action);
     - **interruption**: the app crashing, being killed or losing power halfway. At every point the original data must still exist intact, in the source or the finished target. Write to a temporary file and rename it into place; move across disks as copy, verify, then delete.
   - When in doubt, keep both, skip, or ask. Never overwrite silently.
2. **No telemetry.** The app sends nothing about its user, their files or how they use it: no analytics, crash reports, usage pings or identifiers. The network is used only for what the user explicitly asks for (a server connection, opening a link) and for the update check, which is off by default and only reads the latest release from GitHub.
3. **Security vulnerabilities are fixed.** Every Critical and High CVE/RustSec advisory in the app or its dependencies (Rust crates and bundled C libraries such as libssh2, OpenSSL and unrar) is reviewed and fixed before a release, by upgrading, patching, or removing the dependency. Medium and lower ones are reviewed and fixed when they are reachable from the app. Check with `cargo audit` before tagging (`cargo install cargo-audit`). An advisory may be ignored in `.cargo/audit.toml` only when no fixed version is reachable and the vulnerable code cannot be fed untrusted input; each entry states why and what removes it (currently: quick-xml 0.30 from egui 0.31's Linux accessibility stack, removed by upgrading egui).

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
- Releases: tag `v<version>` from `Cargo.toml`, attach the Apple silicon DMG from `make dmg` (ad-hoc signed, not notarized).
- `README.md` is kept short: the core features for data scientists and developers, not a full feature list. Building, tests, settings/translations and the project layout are in `CONTRIBUTING.md` (add new modules there). `README.html`/`README_files/` are rendered output and git-ignored.
