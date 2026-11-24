# Contributing

## Building

The [install script](scripts/install.sh) is the easy way (see the README). By hand, with
**Rust 1.85+** and a C/C++ compiler (the RAR reader and libssh2 are written in C/C++):

```bash
cargo build --release
```

The binary is written to `target/release/matrix_finder` (`.exe` on Windows).

| Platform | Setup |
|---|---|
| 🐧 **Linux** | Install a C/C++ compiler, pkg-config, the OpenSSL headers and the X11/Wayland libraries egui needs. Debian/Ubuntu: `sudo apt install build-essential pkg-config libssl-dev libxkbcommon-x11-0 libgl1 libegl1`. Fedora: `sudo dnf install gcc gcc-c++ make pkgconf-pkg-config openssl-devel libxkbcommon-x11 mesa-libGL mesa-libEGL` |
| 🍎 **macOS** | Install the Xcode command line tools. For SFTP, use Homebrew OpenSSL (`brew install openssl@3`) or build with `--features vendored-openssl` |
| 🪟 **Windows** | Install the MSVC build tools. SFTP uses the built-in Windows crypto (WinCNG), so OpenSSL is not needed |
| ✂️ **No SFTP** | `cargo build --release --no-default-features` (no OpenSSL or libssh2 at all) |

### macOS app and DMG

```bash
make dmg            # or: scripts/macos/build-dmg.sh
```

This builds `target/macos/Matrix Finder.app` and `target/macos/MatrixFinder-<version>-macos-<arch>.dmg`. Open the DMG and drag **Matrix Finder** onto **Applications**.

- `make dmg-universal` builds one binary for both Apple silicon and Intel (releases ship only the Apple silicon DMG since 0.8.0, so this is the way to a DMG for an Intel Mac), and `make app` builds only the `.app`.
- `--sign "Developer ID Application: …"` applies a real signature. The default is an ad-hoc signature, so macOS asks for confirmation the first time you open the app (right-click → Open).
- `--no-layout` skips the Finder window styling, and `BUNDLE_ID=…` changes the bundle identifier.
- After editing the SVGs in `assets/logo`, run `make icons` to regenerate the PNGs, the `.icns` file and the DMG background.
- `make screenshots` re-renders the screenshots headlessly from a generated demo folder: every scene to `docs/light` and `docs/dark`, plus `docs/showcase.png` and `docs/banner.png` (the showcase under the logo, name, slogan and GitHub link; `SHOTS=panels,palette` renders only some).

## Tests

```bash
cargo test
```

The tests cover:

- archive round-trips for every format
- the filter language
- copy and move
- backups with excludes and rotation
- Markdown parsing and syntax highlighting
- the treemap layout
- checksum digests, list parsing and verification
- every data and model reader (NumPy, HDF5, safetensors, GGUF, PyTorch, ONNX, MATLAB,
  NetCDF, ORC, Stata, SPSS, SAS, R…), with cut-short, damaged and hostile files
- safety: archives that try to write outside their folder, repositories that try to run
  programs through git, terminal links and typed paths
- the terminal's keyboard (kitty protocol) and mouse reports
- translation completeness

Live FTP/SFTP round-trips run against your own servers:

```bash
MATRIX_FINDER_TEST_FTP=127.0.0.1:21:user:pass cargo test live_ftp -- --ignored
MATRIX_FINDER_TEST_SFTP=host:22:user:/path/to/private_key cargo test live_sftp -- --ignored
```

GUI tests drive the real app headlessly (`egui_kittest` + wgpu) and are ignored by default; run them one at a time, as they share a demo folder: `cargo test -- --ignored --test-threads=1 <name>`.

## Code style

The code is formatted with rustfmt, set up in `rustfmt.toml` (long lines: `max_width = 160`
with `use_small_heuristics = "Max"`, stable options only):

```bash
cargo fmt            # or: make fmt
cargo fmt --check    # or: make fmt-check, fails on unformatted code
```

Run it before every commit. `#[rustfmt::skip]` is kept for data tables that read best one
row per line (the shortcut table in `ui/shortcuts.rs`). A commit that only reformats code is
added to `.git-blame-ignore-revs`, so `git blame` looks past it
(`git config blame.ignoreRevsFile .git-blame-ignore-revs`; GitHub does this by itself).

## Settings and translations

- **Settings** are stored in `<config dir>/matrix_finder/settings.json`: `~/.config` on Linux, `~/Library/Application Support` on macOS and `%APPDATA%` on Windows. *File → Export settings…* writes a copy **without passwords**. Saves are atomic (temporary file, flush, rename); the version replaced is kept as `settings.json.bak`, a file that cannot be read is renamed `settings.json.broken-<date>` (never overwritten), a value that cannot be read resets only that setting, and the saves of two running Matrix Finders are merged (`config::Store`).
- **Languages** live in `<config dir>/matrix_finder/lang/<code>.json`. *Settings → Export translation template…* writes every key for you, and missing keys fall back to English.
  ```json
  { "code": "fr", "name": "Français", "strings": { "Copy": "Copier", "Delete": "Supprimer" } }
  ```

## Project layout

Modules are grouped by area. A module depends on the ones it needs and not the other way
round: helpers used in several places go into `util.rs`, not into the first module
that needed them, and the viewers in `preview/` do not depend on the file manager's UI.

```text
src/
├── main.rs            entry point (command line, single instance, window icon)
├── util.rs            small shared helpers: sizes, counts, natural sort, private temp folders, tools on PATH
├── config.rs          settings: the JSON file, defaults, normalize, merged saves (`config::Store`)
├── i18n.rs            translations (+ i18n_data.rs: keys and Vietnamese)
├── app.rs             App: its state, start-up, session, themes and the per-frame update
├── app/               impl App, by topic
│   ├── bars.rs        application menu, top bar, sidebar, status bar, toasts
│   ├── bookmarks.rs   sidebar bookmarks, the bookmarks bar, Settings → Bookmarks
│   ├── commands.rs    `run_cmd`, the command palette, the Checksum and Batch rename tabs
│   ├── connect.rs     server connections, drives and eject
│   ├── dialogs.rs     Copy / Move, New, Rename, Delete, Pack, Unpack, … and what they do; Help, About
│   ├── jobs.rs        background jobs, the Transfers window, dropped files
│   ├── keys.rs        shortcuts, the file list's keys, the type-to-search box
│   ├── opening.rs     opening the cursor's item, Open with, archive passwords, local copies, Quick View
│   ├── settings_ui.rs the Settings tab
│   ├── terminals.rs   terminals in folders or with a profile, their events, AI agents
│   ├── windows.rs     tabs and windows: split, close, reopen, detach, merge, drag
│   └── gui_tests.rs   GUI tests and screenshots, headless with egui_kittest (ignored by default)
├── files/             files and where they live
│   ├── vfs.rs         entries, locations (local / remote / archive), free space
│   ├── archive.rs     list / extract / create for all archive formats
│   ├── remote.rs      FTP (suppaftp) and SFTP (ssh2) back-ends
│   ├── ops.rs         background copy/move/delete with progress, pause, queue and cancel
│   ├── safefs.rs      safe writes: temporary file, sync, no-replace rename into place by the conflict policy
│   ├── tempcopy.rs    files from servers and archives opened through a local copy: edits uploaded or kept
│   ├── drives.rs      mounted drives, a watcher that keeps them current, eject
│   └── gitinfo.rs     git folders in the list: branch from .git/HEAD, status from a hardened git
├── tools/             tools that work on a selection
│   ├── checksum.rs    CRC32 / MD5 / SHA-1 / SHA-256 / SHA-512 / BLAKE3, compare, save and verify lists
│   ├── rename.rs      batch rename rules, validation, safe apply/undo and its tab
│   ├── backup.rs      backup profiles, runner and UI
│   └── diskusage.rs   disk usage window: depth-limited scan, treemap, table, CSV
├── ui/                the file manager's own interface
│   ├── pane.rs        a window: file view (breadcrumb, listing, sort, filter, selection, context menus) or terminal
│   ├── tabs.rs        the tab bar: tabs holding windows, tab groups, drag & drop of tabs and windows
│   ├── layout.rs      tiling split tree of a tab's windows (split, dock, detach, dividers)
│   ├── palette.rs     command palette (Ctrl+K) with fuzzy search
│   ├── completion.rs  folder suggestions for the address bar and folder fields
│   ├── foldertree.rs  the folder tree popup, with OK / Cancel and arrow keys
│   ├── pathfield.rs   a folder field with suggestions and the folder tree button
│   ├── filter.rs      quick filter + SQL-like expression parser
│   ├── chrome.rs      native-looking integrated title bar
│   └── shortcuts.rs   keyboard shortcuts: actions, defaults per system, terminal safety
├── style/             how the app looks
│   ├── theme.rs       colour themes (default: Breeze Light) and terminal colour schemes
│   ├── icons.rs       icon packs and UI line icons
│   ├── sysicons.rs    freedesktop icon themes
│   ├── fonts.rs       bundled and installed fonts, interface and terminal font settings
│   ├── fontfallback.rs system fonts loaded on demand for characters the bundled ones cannot draw
│   └── logo.rs        the logo, drawn from its SVG
├── terminal.rs        terminal profiles (terminal + shell + startup script + colour theme), open-with
├── terminal/view.rs   built-in terminal widget (alacritty_terminal)
├── preview.rs         Quick View: file kinds, size limits, loading in the background, images and text
├── preview/           the viewers, one per family of formats
│   ├── code.rs        syntax highlighting (syntect)
│   ├── markdown.rs    Markdown parsing and drawing
│   ├── widgets.rs     pieces the viewers share: header row, locked-file note, textures
│   ├── sheet.rs       the table every tabular format is read into (Sheet, Cell, TableData)
│   ├── table.rs       Parquet, Avro, Excel/ODS and CSV readers, and which reader opens a file
│   ├── scidata.rs     SQLite, JSON Lines, NumPy, Arrow and HDF5 tables
│   ├── tensors.rs     safetensors, NumPy archives, ONNX, GGUF and PyTorch models: their tensors
│   ├── pickle.rs      Python pickles read as data only (PyTorch checkpoints), never run
│   ├── datameta.rs    MATLAB, NetCDF, ORC, Stata, SPSS, SAS and DuckDB: variables from the header
│   ├── rdata.rs       R data files (RData, RDS)
│   ├── records.rs     calendars, contacts and subtitles as tables
│   ├── docview.rs     PDF (hayro) and Jupyter notebook previews
│   ├── textdocs.rs    RTF, EPUB, ODF, Word and PowerPoint text
│   ├── mail.rs        e-mail messages and mailboxes
│   ├── media.rs       HEIC/AVIF decoding, video details (MP4, Matroska, AVI) and thumbnails
│   ├── rawphoto.rs    camera RAW metadata and embedded previews
│   ├── psd.rs         Photoshop documents: the merged picture
│   ├── audio.rs       audio tags, stream facts and cover pictures (lofty)
│   ├── fontview.rs    font sample sheets
│   ├── model3d.rs     STL, OBJ and glTF models and their renderer
│   ├── geoview.rs     GeoJSON, WKT, WKB, GPX, KML, shapefile and GeoPackage geometry and its drawing
│   ├── jsonview.rs    JSON as a tree, with paths to copy
│   └── certview.rs    X.509 certificates
└── system/            the app and the system around it
    ├── instance.rs    single instance: later launches hand their folder over (localhost + token)
    ├── activation.rs  Linux startup notification: hands the desktop's token to the window
    ├── defaultapps.rs which app opens which file type, and the system's own choice
    ├── agents.rs      AI coding agents for the AI button: which are installed, their logos
    └── update.rs      checking GitHub for a newer release (only when asked)
assets/agents/     logos of the AI coding agents (LobeHub Icons, MIT)
assets/fonts/      Source Sans 3, Source Serif 4, Source Code Pro (SIL OFL 1.1)
assets/logo/       Matrix Finder logo: a folder made of a matrix (SVG source + PNG exports)
assets/macos/      the .icns icon and the DMG background
assets/testdata/   small files the tests read (certificates, encrypted archives)
scripts/           install script, macOS bundle and DMG builder
docs/              README screenshots (light and dark)
```

## Commit messages

```
<type>(<scope>): <summary>

<body: what changed and why, wrapped at 72 columns>

<footer: Closes #12, BREAKING CHANGE: …, Co-Authored-By: …>
```

- **type**: `feat` (new feature), `fix` (bug fix), `perf`, `refactor` (no behaviour change),
  `docs`, `test`, `build` (Cargo, scripts, packaging), `ci`, `chore` (anything else).
- **scope** (optional): the area touched, usually a module: `terminal`, `bookmarks`,
  `archive`, `remote`, `preview`, `settings`, `i18n`, `macos`, …
- **summary**: imperative, lower case, no final period, at most 72 characters
  (`add`, not `added` / `adds`).
- **body** (optional): explain *why* and any user-visible behaviour; skip it when the
  summary says it all.
- A breaking change (settings file, command line, shortcuts) adds `!` after the type
  (`feat(settings)!: …`) and a `BREAKING CHANGE:` footer.
- One logical change per commit; keep translations (`i18n_data.rs`) with the feature
  that needs them.

Examples:

```
feat(bookmarks): cd the terminal to a bookmark when it is focused
fix(archive): keep the cursor on the file after extracting
build(macos): build a universal DMG
```

## Releases

Tags are `v<version>` from `Cargo.toml` (e.g. `v0.1.0`); the GitHub release carries
the Apple silicon DMG from `make dmg` (Intel Macs build from source).
