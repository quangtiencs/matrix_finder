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

## Settings and translations

- **Settings** are stored in `<config dir>/matrix_finder/settings.json`: `~/.config` on Linux, `~/Library/Application Support` on macOS and `%APPDATA%` on Windows. *File → Export settings…* writes a copy **without passwords**. Saves are atomic (temporary file, flush, rename); the version replaced is kept as `settings.json.bak`, a file that cannot be read is renamed `settings.json.broken-<date>` (never overwritten), a value that cannot be read resets only that setting, and the saves of two running Matrix Finders are merged (`config::Store`).
- **Languages** live in `<config dir>/matrix_finder/lang/<code>.json`. *Settings → Export translation template…* writes every key for you, and missing keys fall back to English.
  ```json
  { "code": "fr", "name": "Français", "strings": { "Copy": "Copier", "Delete": "Supprimer" } }
  ```

## Project layout

```text
src/
├── main.rs        entry point (window icon)
├── app.rs         main window, menus, keyboard, dialogs, jobs
├── pane.rs        a window: file view (breadcrumb, listing, sort, filter, selection, context menus) or terminal
├── completion.rs  folder suggestions for the address bar and folder fields
├── foldertree.rs  the folder tree popup, with OK / Cancel and arrow keys
├── pathfield.rs   a folder field with suggestions and the folder tree button
├── layout.rs      tiling split tree of a tab's windows (split, dock, detach, dividers)
├── tabs.rs        the tab bar: tabs holding windows, tab groups, drag & drop of tabs and windows
├── palette.rs     command palette (Ctrl+K) with fuzzy search
├── checksum.rs    CRC32 / MD5 / SHA-1 / SHA-256 / SHA-512 / BLAKE3, compare, save and verify lists
├── terminal.rs    terminal profiles (terminal + shell + startup script + colour theme), open-with
├── termview.rs    built-in terminal widget (alacritty_terminal)
├── vfs.rs         entries, locations (local / remote / archive), free space
├── drives.rs      mounted drives, a watcher that keeps them current, eject
├── remote.rs      FTP (suppaftp) and SFTP (ssh2) back-ends
├── archive.rs     list / extract / create for all archive formats
├── ops.rs         background copy/move/delete with progress, pause, queue and cancel
├── tempcopy.rs    files from servers and archives opened through a local copy: edits uploaded or kept
├── rename.rs      batch rename rules, validation, safe apply/undo and its tab
├── safefs.rs      safe writes: temporary file, sync, no-replace rename into place by the conflict policy
├── filter.rs      quick filter + SQL-like expression parser
├── preview.rs     image / SVG / Markdown / code / text / hex / table previews
├── docview.rs     PDF (hayro) and Jupyter notebook previews
├── table.rs       Parquet, Avro, Excel/ODS and CSV readers
├── diskusage.rs   disk usage window: depth-limited scan, treemap, table, CSV
├── backup.rs      backup profiles, runner and UI
├── icons.rs       icon packs and UI line icons  ·  sysicons.rs  freedesktop icon themes
├── theme.rs       colour themes (default: Breeze Light) and terminal colour schemes
├── fonts.rs       bundled and installed fonts, interface and terminal font settings
├── settings_ui.rs the Settings tab
├── shortcuts.rs   keyboard shortcuts: actions, defaults per system, terminal safety
├── media.rs       HEIC/AVIF decoding, video details (MP4, Matroska, AVI) and thumbnails
├── rawphoto.rs    camera RAW metadata and embedded previews
├── scidata.rs     SQLite, JSON Lines, NumPy, Arrow and HDF5 tables
├── tensors.rs     safetensors, NumPy archives, ONNX, GGUF and PyTorch models: their tensors
├── pickle.rs      Python pickles read as data only (PyTorch checkpoints), never run
├── datameta.rs    MATLAB, NetCDF, ORC, Stata, SPSS, SAS and DuckDB: variables from the header
├── rdata.rs       R data files (RData, RDS)
├── textdocs.rs    RTF, EPUB, ODF, Word and PowerPoint text
├── mail.rs        e-mail messages and mailboxes
├── records.rs     calendars, contacts and subtitles as tables
├── audio.rs       audio tags, stream facts and cover pictures (lofty)
├── psd.rs         Photoshop documents: the merged picture
├── fontview.rs    font sample sheets
├── model3d.rs     STL, OBJ and glTF models and their renderer
├── gitinfo.rs     git folders in the list: branch from .git/HEAD, status from a hardened git
├── geoview.rs     GeoJSON, WKT, WKB, GPX, KML, shapefile and GeoPackage geometry and its drawing
├── jsonview.rs    JSON as a tree, with paths to copy
├── update.rs      checking GitHub for a newer release (only when asked)
├── certview.rs    X.509 certificates
├── defaultapps.rs which app opens which file type, and the system's own choice
├── chrome.rs      native-looking integrated title bar
├── agents.rs      AI coding agents for the AI button: which are installed, their logos
└── i18n.rs        translations (+ i18n_data.rs: keys and Vietnamese)
assets/agents/     logos of the AI coding agents (LobeHub Icons, MIT)
assets/fonts/      Source Sans 3, Source Serif 4, Source Code Pro (SIL OFL 1.1)
assets/logo/       Matrix Finder logo: a folder made of a matrix (SVG source + PNG exports)
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
