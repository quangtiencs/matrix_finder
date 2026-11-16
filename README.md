<div align="center">

<img src="assets/logo/matrix_finder-256.png" width="132" alt="Matrix Finder logo">

# Matrix Finder

### Files, archives and servers, all in one place.

A fast, cross-platform **dual-panel file manager** written in Rust.<br>
Easy to start with, and it has power-user features in the Total Commander tradition.

[![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![egui](https://img.shields.io/badge/UI-egui%200.31-5865F2)](https://github.com/emilk/egui)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-2ea44f)](#build)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](#license)
[![Languages](https://img.shields.io/badge/i18n-English%20%7C%20Ti%E1%BA%BFng%20Vi%E1%BB%87t-ff69b4)](#settings)
[![No telemetry](https://img.shields.io/badge/telemetry-none-success)](#privacy)

[**Features**](#features) ·
[**Screenshots**](#screenshots) ·
[**Shortcuts**](#shortcuts) ·
[**Download**](#download) ·
[**Build**](#build) ·
[**Privacy**](#privacy) ·
[**Project layout**](#layout)

<br>

<img src="docs/showcase.png" alt="Matrix Finder in the Breeze Light theme: dual panels, light files beside a dark terminal in one tab, a Parquet preview, and a disk usage treemap">

<sub>Dual panels · light files beside a dark terminal in one tab · Parquet preview · disk usage treemap, in the <b>Breeze Light</b> theme with a <b>Breeze Dark</b> terminal.</sub>

</div>

---

## ✨ Highlights

<table>
<tr>
<td width="33%" valign="top">

### 🗂️ Tabs & split windows
One Chrome-style tab bar with **tab groups**, pinning and *reopen closed tab*. Each tab splits into up to 2 × 2 file and terminal windows. You can drag a window to swap or dock it, or pull it out into its own tab.

</td>
<td width="33%" valign="top">

### ⌨️ Command palette
<kbd>Ctrl</kbd>+<kbd>K</kbd>: fuzzy search across every command, bookmark, drive, theme, open tab and saved connection.

</td>
<td width="33%" valign="top">

### 🔐 Checksums <sup>new</sup>
CRC32, MD5, SHA-1, SHA-256, SHA-512 and BLAKE3. Paste a hash to **compare**, save `.sha256` lists or **verify** `SHA256SUMS`.

</td>
</tr>
<tr>
<td valign="top">

### 💻 Built-in terminal
GPU-drawn, full xterm emulation, 10,000 lines of scrollback and clickable links. **Profiles** pick the shell, a startup script and a **colour theme**.

</td>
<td valign="top">

### 📦 Archives as folders
Browse zip, 7z, rar and tar.* like folders, preview the files inside, and create zip, 7z or tar.zst in the background.

</td>
<td valign="top">

### 🌐 Remote servers
**SFTP** (password, key or agent) and **FTP**. Type `sftp://user@host/path` straight into the path bar.

</td>
</tr>
<tr>
<td valign="top">

### 👁️ Quick View
Previews for PDF, **Word / PowerPoint**, Jupyter notebooks, images (PSD, EXR, TGA…), **audio** with its cover, SVG, Markdown, code in ~60 languages, **JSON as a tree** with paths to copy, plus **Parquet / Avro / Excel / CSV / NumPy / HDF5 / R** tables, **safetensors / ONNX / GGUF / PyTorch** model weights, **MATLAB / NetCDF / Stata / SPSS / SAS / DuckDB** variables and **GeoJSON / GPX / KML / shapefile** geometry.

</td>
<td valign="top">

### 🔎 SQL-like filter
`size > 10MB AND ext IN ('mp4','mkv')`: filter the panel live as you type, or search every subfolder. Search inside files like grep (`source LIKE '%TODO%'`) and match names with regular expressions.

</td>
<td valign="top">

### 🎨 Native looks
**Breeze Light** (default), Adwaita, macOS, Windows 11, Elegant, Dracula, Nord, Catppuccin… or *System* to follow your OS.

</td>
</tr>
</table>

## 🆕 What's new

| Feature | Details |
|---|---|
| 📄 **Office documents** <sup>0.9</sup> | **Word** (`.docx`) with headings, lists and tables, **PowerPoint** (`.pptx`) and ODF presentations slide by slide, and the picture saved in **Pages, Keynote and Numbers** files |
| 🎵 **Audio** <sup>0.9</sup> | MP3, FLAC, WAV, Ogg, Opus, M4A, AIFF and more: title, artist, album, track, duration, bitrate and sample rate, with the cover picture. No sound is decoded |
| 🧠 **GGUF and PyTorch** <sup>0.9</sup> | llama.cpp's **GGUF** models and **PyTorch** checkpoints (`.pt`, `.pth`, `.ckpt`, `pytorch_model.bin`) list every tensor with its shape, type and size, plus their metadata. Only the header is read; a checkpoint's pickle is read as data and never run |
| 🧪 **Scientific and statistical data** <sup>0.9</sup> | **MATLAB**, **NetCDF**, **ORC**, **Stata**, **SPSS**, **SAS** and **DuckDB** files list their variables or columns (names, shapes, types, labels) from their header, so files of any size open at once |
| 🗺️ **More geometry** <sup>0.9</sup> | **GPX** tracks, **KML / KMZ**, ESRI **shapefiles** and **GeoPackage** are drawn like GeoJSON |
| ✉️ **Mail, calendars, contacts, subtitles** <sup>0.9</sup> | `.eml` and `.mbox` messages with their attachments listed, `.ics` events, `.vcf` contacts and `.srt` / `.vtt` subtitles as tables; comic books (`.cbz`, `.cbr`, `.cb7`) show their cover |
| 🔔 **Check for updates** <sup>0.8</sup> | *Check for updates…* asks GitHub for the latest release and shows what is new, with links to download it; you install it yourself. Checking at start is a setting, off by default |
| 🌲 **JSON tree** <sup>0.8</sup> | JSON files open as a tree in Quick View. Click a row to get its path, `j['data'][0]['name']`, and copy it in Python, JavaScript, jq or JSONPath style; right-click to copy the value |
| 🔍 **Search inside files** <sup>0.8</sup> | `source LIKE '%TODO%'` in the search box finds files with a matching line, like grep, in the folder or (<kbd>↓</kbd> <kbd>Enter</kbd>) every subfolder. Names, extensions and lines also take **regular expressions**: `name REGEXP '^img_\d+'`, `name ~ '^IMG'`, `source ~ 'fn \w+'` |
| 🔷 **Geometry preview** <sup>0.8</sup> | Quick View draws the shapes in **GeoJSON**, **WKT** and **WKB** files (points, lines, polygons with holes, multi-geometries and collections) on a plain grid, with no map and nothing fetched. Drag to move, scroll to zoom, and *Source* shows the text |
| 💽 **Drives & Eject** <sup>0.7</sup> | The sidebar's drives update by themselves when one is plugged in or removed. External and network drives have an **Eject** button, refused while a task still uses the drive. *Settings → Files & layout → Drives in the sidebar*: always, never, or only with more than one drive |
| 🌳 **Folder tree & suggestions** <sup>0.7</sup> | The address bar and every field that takes a folder (Copy / Move, Compress, Extract, Backup, bookmarks) suggest folders as you type and have a **folder tree** popup with OK / Cancel and arrow keys. `~` and relative paths work everywhere |
| 📦 **Compress dialog** <sup>0.7</sup> | Formats as chips with a line on each, the name with its extension, a *Faster ↔ Smaller* slider and where the archive goes. Big archives show their progress in **Tasks** |
| ⚡ **Several quick filters at once** <sup>0.7</sup> | Tick *Images* and *Videos* together in the funnel menu; add your own lists in *Settings → Quick filters* |
| 🧰 **Tools in tabs** <sup>0.7</sup> | Checksum and Batch rename open in tabs. <kbd>F3</kbd> shows or hides Quick View, <kbd>F4</kbd> opens a folder here and a file in its app |
| ⇅ **Tasks in a tab** | Copies, moves, compressing, extracting and backups show in the Tasks tab (formerly Transfers), and starting one never opens it for you. While they run, the toolbar button becomes a **circular progress indicator** |
| 🚪 **Quit confirmation** | Quitting while jobs are running or queued asks first: keep transferring, or stop them and quit |
| 🔑 **SFTP host key check** | Servers are verified before any password or key is used. A new server shows its fingerprint for you to trust, and a changed key is refused |
| 🛟 **Safer copies and moves** | An existing file is only replaced once the new copy is complete. Moves with *Skip* keep the files they skipped, and copying out of an archive follows your *If file exists* choice |
| 🔐 **Checksums** | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd>, or right-click → *More* → *Checksum…*, opens in a tab of its own. Hash files in the background, compare with a pasted value (the algorithm is picked from its length), save checksum files and verify existing ones |
| ➕ **Right-click → New** | A *New ▸ Folder… / Blank file…* submenu in every context menu: on a file, on the `..` row and on empty space |
| 🎨 **Breeze Light by default** | Breeze Light is the default theme for the app, and Breeze Dark for built-in terminal profiles |
| 🏠 **Starts at Home** | The app opens your home folder with no terminal running. To restore the last session instead, turn off *Settings → Files & layout → Open the home folder at start* |
| ↺ **Reset to defaults** | *Settings → Reset to defaults…* restores every preference (theme, language, UI scale, panels, terminal and safety options) after you confirm. Bookmarks, saved connections, backup and terminal profiles are kept |
| 👁️ **Quick View stays out of terminals** | While a terminal tab is active, the Quick View panel is hidden so the terminal gets the full width. It comes back when you switch to a file tab, and the setting itself is not changed |
| 🤖 **AI button** | When Claude Code, Codex, Gemini CLI, Qwen Code or OpenCode is installed, the terminal's info bar gets an AI button that starts it at the prompt (only while the terminal is idle). *Settings → Terminal* chooses which tool it starts, or hides it |
| 🖌️ **Terminal colour themes** | 21 built-in schemes with their own ANSI colours (Dracula, Nord, Tokyo Night, Catppuccin, Solarized, Gruvbox…), picked per profile from a gallery with a live sample; open terminals update at once |
| 🚀 **Default apps** | Files open with your system's default app. *Settings → Default apps* or **Always use this app** in Open with lets you choose another one per file type, and one click goes back to the system's choice |
| 🔤 **Fonts** | *Settings → Fonts*: pick the font and size of the interface and of the terminal, from the bundled fonts or anything installed |

<div align="center">
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-json.png"><img src="docs/light/quickview-json.png" width="49%" alt="Quick View of a JSON file as a tree with the path of a row"></picture>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-geometry.png"><img src="docs/light/quickview-geometry.png" width="49%" alt="Quick View of a GeoJSON polygon with a hole"></picture>
<br>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/folder-tree.png"><img src="docs/light/folder-tree.png" width="49%" alt="Folder tree popup and drives with an Eject button"></picture>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/compress.png"><img src="docs/light/compress.png" width="49%" alt="Compress dialog"></picture>
<br>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/checksum.png"><img src="docs/light/checksum.png" width="49%" alt="Checksum tab comparing a pasted SHA-256"></picture>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/checksum-verify.png"><img src="docs/light/checksum-verify.png" width="49%" alt="Verifying a SHA256SUMS file"></picture>
<br>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/new-menu.png"><img src="docs/light/new-menu.png" width="49%" alt="Right-click New submenu"></picture>
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/terminal.png"><img src="docs/light/terminal.png" width="49%" alt="Built-in terminal"></picture>
</div>

<a id="features"></a>

## 🚀 Features

<details open>
<summary><b>🖥️ Interface and window</b></summary>

- **Clean, minimal layout** in the Finder / Nautilus style. A full-height sidebar holds the app menu (the button just before it, or *View → Sidebar* in that menu, hides and shows the sidebar), bookmarks (or put them in a bar above the tabs), drives and network connections. There is a single toolbar, rounded panel cards and a clickable breadcrumb path (click the empty area or press <kbd>Ctrl</kbd>+<kbd>L</kbd> to type a path). File icons are vector drawings coloured by type, and a slim status bar shows clickable key hints.
- **Address suggestions**: while you type a path, the folders and archives that match appear below the address bar, names that start with what you typed first. <kbd>↑</kbd> <kbd>↓</kbd> pick one and <kbd>Enter</kbd> opens it, a click opens it too, and <kbd>Tab</kbd> completes the name (or as much as the matches share) so you can go on typing inside it. `~/` works, a name on its own is looked up in the window's folder, and hidden folders show up when hidden files are shown or once you type a dot.
- **Folder tree**: the tree button at the start of the address bar opens a small tree of your home folder and drives, already open down to the current folder. A chevron opens or closes a folder (read in the background, so a slow drive never holds up the window). A click chooses a folder (or <kbd>↑</kbd> <kbd>↓</kbd>, with <kbd>←</kbd> <kbd>→</kbd> to close and open), and **OK**, <kbd>Enter</kbd> or a double click goes there. **Cancel**, <kbd>Esc</kbd> or a click elsewhere closes it.
- **Folder fields**: every field that takes a folder suggests folders as you type, like the address bar, and has the folder tree button beside it: *To* in Copy / Move (<kbd>F5</kbd> / <kbd>F6</kbd>), *Save to* in Compress, *Extract to*, the backup's sources, destination and *Restore to*, and the bookmarks' locations. <kbd>↑</kbd> <kbd>↓</kbd> and <kbd>Enter</kbd> or a click fill a suggestion in and list the folders inside it, <kbd>Tab</kbd> completes, and <kbd>Esc</kbd> closes the list without closing the dialog; <kbd>Enter</kbd> with no suggestion chosen confirms the dialog.
- **Drives** in the sidebar go by their names (*Macintosh HD*, *KINGSTON*, *Windows (C:)*; hover for the path), without the system's own mounts such as macOS's Data volume. The section shows only when there is more than one drive; *Settings → Files & layout → Drives in the sidebar* shows it always or never. The list follows the system: a drive that is plugged in, ejected or unmounted elsewhere appears or leaves within a couple of seconds, and a window showing a drive that went away goes to your home folder. External, network and disk image volumes have an **eject** button (also in their right-click menu). It runs in the background (`diskutil` on macOS; `gio`, `udisksctl` or `umount` on Linux; Explorer's Eject on Windows). While a task uses the drive, ejecting is refused; if a program still has files open there, the message names it.
- **Native window chrome**: the title bar is part of the toolbar and uses the platform's own window buttons.
  - macOS keeps its real traffic lights in a 52 pt unified title bar.
  - Windows gets Windows 11 caption buttons.
  - GNOME gets Adwaita round buttons in the order set by `button-layout`.
  - KDE gets Breeze buttons.
  - To switch back to the system title bar, turn off *Settings → Files & layout → Integrated title bar*.
- **One tab bar, split windows**: there is a single tab bar, and each tab holds one or more windows. A window shows either files or a terminal.
  - **Splitting**: use right-click → **Layout**, or the split button in a window's title row. You can split **right / left / down / up** with the same kind of window, or add a terminal beside files (or files beside a terminal). A tab holds up to a 2 × 2 grid.
  - **Resizing and focus**: drag the dividers to resize them, and double-click a divider to reset it to 50/50. <kbd>Tab</kbd> moves between file windows. The tab's icon shows a miniature of its layout.
  - **Moving windows**: drag a window by its **⠿ grip**. Drop it in the middle of another window to **swap** the two, or on another window's edge to **dock** beside it. Drop it on the **tab bar** to give it a tab of its own, or onto another tab to move it into that tab.
  - **Merging tabs**: drag a tab onto a window's edge to merge the two tabs, or use the tab menu's *Show beside the current tab*. The palette's *Move window to new tab* splits a window back out.
  - <kbd>Ctrl</kbd>+<kbd>W</kbd> closes the focused window, and closes the tab when it has only one window. <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd> closes the whole tab, all its windows included.
  - **Settings** (<kbd>Ctrl</kbd>+<kbd>,</kbd> or the ⚙ button) opens in a tab of its own, not a separate window. Opening it again switches to that tab.
  - **Tasks** (<kbd>Ctrl</kbd>+<kbd>J</kbd> or the ⇅ button) works the same way.
  - **Checksum** and **Batch rename** open in a new tab next to the current one, each time, so several can run side by side. Closing the tab stops what it is still hashing.
- **Tabs & tab groups**: you can reorder, pin, rename and duplicate tabs (a split tab is duplicated with all its windows), use *close others / close to the right*, and **reopen closed tabs** (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd>). Groups have names and 9 colours and can be collapsed. Split tabs and their layout are restored with the session.
- **Startup**: the app opens in your **home folder** with no terminal open. Turn off *Open the home folder at start* to restore the tabs and groups of the last session instead.

</details>

<details>
<summary><b>📁 Files, drag & drop and tasks</b></summary>

- **Right-click → New ▸ Folder… / Blank file…** (<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F4</kbd>).
- **Drag & drop**: drop files on the other panel, a folder row, the `..` row, a breadcrumb segment, a bookmark or a drive.
  - A floating label shows what will happen ("Copy → docs").
  - Hold <kbd>Shift</kbd> to move instead of copy, and press <kbd>Esc</kbd> to cancel.
  - Hovering over a folder opens it (spring-loaded folders), and the list scrolls when you drag near its edges.
  - Files dragged in from the OS land in the panel under the pointer.
- **Tasks** (<kbd>Ctrl</kbd>+<kbd>J</kbd>, formerly *Transfers*): every copy, move, delete, compress, extract and backup runs in the background, and you keep browsing. It opens in a tab when you ask for it, never by itself; the status bar shows the running task with its progress bar meanwhile.
  - The toolbar button turns into a **circular progress indicator** showing how far the running jobs are together, with a count when there are several.
  - Each job shows progress, speed and time left, with **pause / resume / cancel**.
  - Jobs wait in a **queue**, and you choose how many run at once.
  - Finished jobs stay in a history list until the app closes.
  - **Quitting asks first** while jobs are running or queued, including <kbd>⌘</kbd>+<kbd>Q</kbd> and the Dock's Quit on macOS. *Stop and quit* cancels the jobs and gives them up to 10 seconds to clean up.
- **If a file already exists**, a copy or move can keep both, overwrite or skip.
  - A move only deletes each source file once it has been copied, so skipped files stay where they were.
  - A cancelled or failed copy to a local folder never leaves a half-written file behind, and an existing file stays intact until the new copy is complete. An interrupted upload can still leave a partial file on the server.
- **Batch rename** (<kbd>Ctrl</kbd>+<kbd>M</kbd>, in a tab of its own):
  - Name masks with the tokens `[N]` `[N2-5]` `[E]` `[C]` `[D]` `[T]` `[Y]` `[M]` `[d]` `[P]`.
  - Find & replace (plain text or regex with `$1` groups), letter case, **Vietnamese diacritics removal** and a slug mode.
  - A live preview flags conflicts, swaps such as a↔b are safe, and the last batch can be undone.
- **Open with**: <kbd>Enter</kbd> / <kbd>F4</kbd> opens a file with the OS default app (<kbd>F4</kbd> on a folder opens it right here, as <kbd>Enter</kbd> does), *Open with…* picks any program, and *Show in file manager* reveals it. Remote and in-archive files are fetched to a temp folder first.
- **Bookmarks** (<kbd>Ctrl</kbd>+<kbd>D</kbd>): Home, Desktop, Documents, Downloads, Music, Pictures and Movies are bookmarked by default.
  - **Two display modes**: a **vertical bar** in the sidebar (the default), or a **horizontal bar** above the tabs, like a browser's bookmarks bar. Bookmarks that don't fit in the horizontal bar go into a **»** menu.
  - **In a terminal**: choose in Settings → Bookmarks whether a bookmark always opens in a file window (**Go to file only**) or, when a terminal is focused, runs `cd` to the folder in its shell (**File and Terminal**, the default).
  - **Icons**: standard folders get their own icons, or the desktop icon theme's when one is in use. You can switch to plain folder icons, or pick an icon for any bookmark.
  - **Folders**: group bookmarks into folders. Folders open and close in the sidebar and open as a menu in the horizontal bar.
  - **Managing bookmarks**: use *Settings → Bookmarks* to rename and reorder bookmarks, change their location and icon, move them into folders, and import or export JSON.
- **Safety**: deleting goes to the trash (<kbd>Shift</kbd>+<kbd>Del</kbd> deletes permanently), there is a properties dialog, and confirmations can be turned off.

</details>

<details>
<summary><b>🔐 Checksums</b></summary>

- **Algorithms**: CRC32, MD5, SHA-1, **SHA-256** (default), SHA-512 and BLAKE3. Folders are walked recursively, and hashing runs in a background thread with progress, speed and cancel.
- **Compare with**: paste the published hash. The algorithm is chosen from its length, and the matching file turns ✔ green. If a hash is already on the clipboard, it is filled in for you.
- **Click any hash to copy it**, or use *Copy all* to copy every line in `sha256sum` format.
- **Save checksum file…** writes `.md5`, `.sha1`, `.sha256`, `.sha512` or `.b3` in GNU format (`.sfv` for CRC32), with paths relative to the list.
- **Verify**: run the command on a checksum file to check every file it lists, with a result of **OK / failed / missing**. Recognised files:
  - `*.sha256`, `*.md5sum` and the other `*.<algorithm>` names
  - `SHA256SUMS` and similar names
  - `*.sfv`
  - BSD-style lines (`SHA256 (file) = …`)

</details>

<details>
<summary><b>💻 Built-in terminal and profiles</b></summary>

- **A terminal in a tab** (<kbd>Ctrl</kbd>+<kbd>`</kbd>), in the style of Ghostty:
  - GPU-drawn, with full VT/xterm emulation (alacritty_terminal): true colour, alternate screen, mouse reporting for vim/htop/tmux, and bracketed paste.
  - 10,000 lines of scrollback. Scrolling follows the finger on a trackpad and moves 3 lines per mouse wheel notch, like Ghostty; both speeds can be changed in *Settings → Terminal*.
  - Copy & paste with word, line and block selection.
  - **Links** (including OSC 8 hyperlinks) are highlighted, and <kbd>Ctrl</kbd>+click opens them.
  - Background tabs show activity and bell dots.
  - Closing a window, a tab or the app while a program (not the shell) is running in a terminal asks first and names the program; this can be turned off in *Settings → Terminal*.
  - An **info bar** on every terminal window shows what is running and the folder, shortened to `~/…` (hover over it for the full path). Its buttons open the folder in a file window beside the terminal, copy the path, clear the screen, restart a shell that has exited, change the text size, and split or close the window.
  - **AI button**: Matrix Finder looks for Claude Code (`claude`), Codex (`codex`), Gemini CLI (`gemini`), Qwen Code (`qwen`) and OpenCode (`opencode`), the way your shell finds them (so PATH and aliases from its startup files count). When one is installed, the info bar gets a single AI button with that tool's logo; a click types its command at the prompt. The button is greyed out while any program runs in the terminal and works again once the shell is idle. *Settings → Terminal → AI button* chooses the tool (or **Automatic**: the first one installed, in the order above), looks again after you install one, and turns the button (and the search) off; right-click the button to switch between the installed tools or hide it.
- **Profiles**: <kbd>F9</kbd> opens the default profile, and the **▾** arrow next to the terminal button lists every profile. A profile combines:
  - **a terminal**: the built-in tab, the native OS terminal, Ghostty, iTerm2, Windows Terminal, PowerShell, Alacritty, Kitty, WezTerm, Konsole, GNOME Terminal, or a custom command with `{dir}` / `{cmd}`
  - **a shell**: bash, zsh, fish, nu, pwsh, cmd and so on
  - **a startup script** (built-in terminal only), for example `source .venv/bin/activate`. It is typed at the prompt once the shell has started, line by line, so whatever your shell's startup files set up (PATH, conda, nvm, aliases, functions) is there to use; restarting the terminal runs it again
  - **a colour theme**: one of 21 schemes (Breeze Dark by default) or the same as the app
- **Fonts**: JetBrains Mono by default, or any monospaced font you pick in *Settings → Fonts*, with Nerd Font symbols as a fallback, so Powerline, powerlevel10k and starship prompts display correctly.

</details>

<details>
<summary><b>📦 Archives, 🌐 servers, 💾 backups and 📊 disk usage</b></summary>

- **Open** zip (jar/apk/cbz/whl), **7z**, **rar**, tar, tar.gz, tar.xz, tar.zst, tar.bz2, gz, xz, zst and bz2. You can browse inside, preview entries and extract everything or only the selection.
  - Copying files out of an archive follows the *If file exists* choice. The Extract dialog and backup restore overwrite existing files, except that a single-file archive such as `report.txt.gz` never replaces a file beside it.
  - A crafted tar archive can't write outside the destination folder through symbolic or hard links.
- **Create** zip, **7z** (LZMA2), tar, tar.gz, tar.xz, **tar.zst** (multithreaded), tar.bz2 and single-file gz/xz/zst/bz2. RAR archives can be created when the `rar` command-line tool is installed.
  - The **Compress** dialog (<kbd>Alt</kbd>+<kbd>F5</kbd>) shows the formats as buttons, with a line on what the one chosen is good for. The single-file formats are offered only for one file, and typing a format at the end of the name (`photos.7z`) chooses it.
  - A *Faster ↔ Smaller* slider sets the level (plain .tar has none). The file it will make is shown before it is made, with a warning when one with that name is there already.
  - Large archives are made in the background: the progress bar is in the status bar and in **Tasks**, where they can be paused.
- **Protocols**: the local disk, **SFTP** (password, private key or SSH agent) and **FTP**, with saved connection profiles.
  - **SFTP host keys** are checked before any password or key is used, against `~/.ssh/known_hosts` and the app's own `known_hosts` file in its settings folder.
  - A server not listed in either shows its SHA-256 fingerprint and connects only if you choose to trust it. Trusted keys go in the app's file, and `~/.ssh` is never changed.
  - A changed key is refused. If the change is expected, remove the old key from whichever of the two files holds it. The error names both.
- **Backups (no cloud)**: profiles with several sources, glob excludes, a format, a file-name template (`{name}-{date}`) and *keep the last N*. Each backup is a normal archive with a manifest, and restore is built in.
  - A backup that can't read one of its folders fails and creates nothing, so an incomplete archive can never replace a good one. Exclude the folder or fix its permissions.
  - *Keep the last N* only removes files named exactly like that profile's backups, so other profiles and your own files in the same folder are safe.
- **Disk usage**: right-click a folder → **Analyze disk usage** (or a drive in the sidebar, or the toolbar button) to open the analysis **in a new tab**, as a window you can split and move like any other.
  - **Depth**: the scan keeps **1** folder level by default. Choose more levels or *All levels* in the info bar; sizes always include everything below.
  - **Views**: show the result as a **treemap** or as a **table** sorted by name, size or file count, with each entry's share of the folder.
  - **Navigation**: click a folder to open it, and use ↑ (or <kbd>Backspace</kbd>) to go up.
  - **Saving**: save the table as **CSV** with *Save as CSV…*.
  - **In the panels**: *Calculate folder sizes* shows folder sizes in the file lists.

</details>

<details>
<summary><b>👁️ Preview and 🔎 search</b></summary>

- **Quick View** (<kbd>F3</kbd> shows and hides it; also <kbd>Ctrl</kbd>+<kbd>Q</kbd>) or the viewer window (<kbd>Shift</kbd>+<kbd>F3</kbd>) shows:
  - **PDF**
  - **Jupyter notebooks**
  - images: PNG, JPEG, GIF, WebP, BMP, ICO, TIFF, TGA, QOI, PNM, DDS, OpenEXR and Radiance HDR, with animated GIF and WebP playing; *Details* shows a photo's EXIF facts (camera, exposure, location)
  - **Photoshop** (PSD, PSB): the merged picture the file keeps, read in place so the layers are stepped over
  - **HEIC/HEIF and AVIF** (macOS decodes them itself; elsewhere libheif, ImageMagick or FFmpeg is needed)
  - **camera RAW** (CR2, NEF, ARW, DNG): the preview the camera embedded, with its exposure details
  - **video** (MP4, MOV, MKV, WebM, AVI, and with FFmpeg also WMV, FLV, MPEG, M2TS, 3GP and Ogg video): duration, resolution and codecs, plus a thumbnail (FFmpeg, or Quick Look for MP4 and MOV on macOS)
  - **audio** (MP3, FLAC, WAV, Ogg Vorbis, Opus, M4A/M4B, AAC, AIFF, Monkey's Audio, WavPack, Musepack, Speex): its tags, duration, bitrate, sample rate, channels and bit depth, with the cover picture
  - **SVG**
  - **Markdown** (rendered or source)
  - **highlighted code** in ~60 languages
  - **JSON** (`.json`, `.jsonld`, `.har`, `.webmanifest`) as a **tree**: keys in the file's order, values coloured by type, objects and arrays with their size. A click on the chevron (or a double click) opens and closes a branch, and *Expand all* / *Collapse all* do the whole tree. Click a row to see its **path** above the tree and copy it with *Copy path*, as Python `j['data'][0]['name']`, JavaScript `j.data[0].name`, jq `.data[0].name` or JSONPath `$.data[0].name` (the style you pick is kept). Right-click a row to copy its path, its value (as indented JSON) or its key. Big arrays show 1,000 items at a time, and *Source* shows the highlighted text. Files over 32 MB, or that are not quite JSON (comments, trailing commas), are shown as text
  - **data tables** for Parquet, Avro, Excel/ODS, CSV/TSV, **SQLite**, **JSON Lines**, **NumPy** (`.npy`, `.npz`), **Arrow/Feather** and **HDF5**
  - **model weights**: **safetensors** and **ONNX** list every tensor with its shape, type and size, and the parameter count; only the header is read, so multi-gigabyte models open at once. ONNX also shows inputs and outputs, operators and metadata
  - **R data** (`.RData`, `.rda`, `.rds`, packed or not): each object with its class and size, and data frames, matrices and vectors as tables; functions and environments are never run
  - **GGUF** models: every tensor with its shape, GGML type (Q4_K, F16…) and size, the parameter count and the metadata (architecture, context length, the tokenizer's first tokens)
  - **PyTorch** checkpoints (`.pt`, `.pth`, `.ckpt`, `pytorch_model*.bin`, the ZIP format and the older one): each tensor by its path in the checkpoint, with shape, type and size, and plain values beside them (epoch, learning rate). The pickle that describes them is read as data only: no class is imported and no code runs
  - **MATLAB** (`.mat` v5, v7 and v7.3), **NetCDF** (classic, 64-bit, CDF-5 and NetCDF-4), **ORC**, **Stata** (`.dta` 113–119), **SPSS** (`.sav`, `.zsav`) and **SAS** (`.sas7bdat`): their variables or columns with shape or type, format and label, and the row count; only the header is read. A file in one of these formats that is really text (G-code in a `.nc`, Octave text in a `.mat`) is shown as text
  - **DuckDB** databases: their tables and views with columns, types and estimated row counts, through the `duckdb` tool when it is installed (opened read-only, with extensions and other files kept out); without it, the header's facts
  - **RTF, EPUB and ODF text** as readable text, with EPUB chapters
  - **Word** (`.docx`): headings, lists, tables and emphasis; **PowerPoint** (`.pptx`) and **ODF presentations** (`.odp`) slide by slide, each slide's title naming it
  - **Pages, Keynote and Numbers**: the picture the document was saved with (a `.key` that is not a Keynote file is shown as a key, never its contents)
  - **e-mail**: `.eml` messages (headers, the text or the HTML as text, attachments by name and size) and `.mbox` mailboxes message by message (the first 1,000, from the first 32 MB). Attachments are never opened and nothing is fetched
  - **calendars** (`.ics`: events, tasks), **contacts** (`.vcf`) and **subtitles** (`.srt`, `.vtt`) as tables, with *Source* for the text
  - **comic books** (`.cbz`, `.cbr`, `.cb7`): the cover and the number of pages; <kbd>Enter</kbd> browses inside
  - **fonts** (TTF, OTF, WOFF, WOFF2): a sample sheet with names and properties
  - **3D models** (STL, OBJ, glTF, GLB): drag to turn, scroll to zoom
  - **geometry** (GeoJSON, GeoJSON sequences `.geojsonl`, WKT and EWKT, WKB and EWKB, binary or as hex text, GPX tracks, routes and waypoints, KML and KMZ, ESRI shapefiles `.shp`, GeoPackage `.gpkg` read in place): points, lines and polygons with their holes, drawn on a plain grid with x and y at the same scale. There is no map, and nothing is fetched. Drag to move, scroll or pinch to zoom around the pointer, double-click to see it all again; the pointer's coordinates show in the corner. The header counts features, geometry types and vertices (and the SRID, if the file has one); *Source* shows the text of GeoJSON and WKT
  - **certificates** (PEM, CRT, CER, DER): validity, names, key and fingerprints; private keys are never displayed
  - a hex dump for binary files
  - folder and archive summaries

  Previews also work inside archives and on SFTP/FTP.

  **Password-protected files:** ZIP (ZipCrypto and AES), 7z and RAR ask for the password when you open, preview or extract something protected. It is checked first, kept in memory until the app closes, and never saved. Protected Office, PDF, ODF and EPUB files show a small *Password protected* message: press <kbd>Enter</kbd> to open them in their own app.

  Files past a size limit are not opened at all, and show a small *Too large to preview* message: 80 MB for images, PDF, notebooks, fonts, certificates, geometry, e-mail and RTF (JSON trees up to 32 MB, then text), 256 MB for `.xls`, `.xlsb` and `.ods`, 768 MB for 3D models, and 1 GB for everything else (Photoshop files, GeoPackages and mailboxes included, which are read in place). Local safetensors, ONNX, `.npz`, GGUF, PyTorch, MATLAB, NetCDF, ORC, Stata, SPSS, SAS and DuckDB files have no limit, since only their headers are read. A header that claims more than any real file has (millions of tensors, a struct inside itself) gets a *too complex to preview* message instead. On a server or inside an archive, only text is previewed past what can be fetched (16 MB and 80 MB).
- **Search & filter**: one box for files and commands. Start typing in a file window to open it with what you typed, or press <kbd>Ctrl</kbd>+<kbd>F</kbd>. As you type, the active panel is filtered live; <kbd>Enter</kbd> keeps the filter and <kbd>Esc</kbd> closes the box and shows the whole folder again. <kbd>↓</kbd> <kbd>Enter</kbd> searches all subfolders instead. Queries can be plain text, globs, extension lists or an **SQL-like WHERE clause**, with **regular expressions** (`name REGEXP '^img_\d+'`) and a **search inside files** like grep (`source LIKE '%TODO%'`), see [Quick filter syntax](#filter).
- **Quick filters by file type**: the **funnel** button of a file window opens a menu of categories with tick boxes: tick one or several (say *Images* and *Videos*) to show only those kinds of file. The menu stays open while you tick, and the chip in the address bar names what is shown. The built-in categories are Images, Music, Videos, Documents, Spreadsheets, Presentations, Development, Data and Archives, each with its common formats.
  - **While filtering**: folders stay visible, and the filter stays on while you browse into other folders. It works together with the text filter. Click the chip in the address bar or press <kbd>Esc</kbd> to turn it off.
  - **Your own lists**: in *Settings → Quick filters* (or *Edit quick filters…* at the bottom of the menu) you can add your own categories, rename, reorder and remove them, edit their extension lists, or restore the built-in ones. The palette lists every category too, and ticks or unticks it.

</details>

<details>
<summary><b>🎨 Themes, icons, fonts and languages</b></summary>

- **Themes**:
  - **Breeze Light** (default) and Breeze Dark
  - Elegant Dark / Light
  - macOS, GNOME Adwaita and Windows 11, each in light and dark
  - **System**, which follows your OS look and its light/dark setting
  - Digital Rain, Classic Commander, Dracula, Nord, Solarized, Gruvbox, Monokai, Catppuccin Mocha, Tokyo Night and GitHub Light

  A UI-scale slider is also available.
- **Icons**: *Match theme*, *System icon theme*, or any installed freedesktop icon theme (Papirus, Humanity…) using its real SVG/PNG icons.
- **Fonts**: Adobe Source Sans 3, Source Serif 4 and Source Code Pro are bundled (SIL OFL), with a CJK fallback. *Settings → Fonts* sets the interface font and text size separately from the terminal's, using the bundled fonts or any installed one. Markdown, notebooks and documents in Quick View are read in Source Sans 3; *Settings → Fonts → Reading* switches them to Source Serif 4.
- **Languages**: English and **Tiếng Việt** are built in. To add another language, drop a JSON file into the language folder.

</details>

<a id="screenshots"></a>

## 📸 Screenshots

The images follow your light or dark GitHub theme. Both sets are in [`docs/light`](docs/light) and [`docs/dark`](docs/dark).

<table>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/panels.png"><img src="docs/light/panels.png" alt="Dual panels"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/native-themes.png"><img src="docs/light/native-themes.png" alt="Native looks"></picture></td>
</tr>
<tr>
<td align="center"><sub>Dual panels</sub></td>
<td align="center"><sub>Native looks: Adwaita, Breeze, macOS, Windows 11</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/palette.png"><img src="docs/light/palette.png" alt="Command palette"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/markdown.png"><img src="docs/light/markdown.png" alt="Markdown quick view"></picture></td>
</tr>
<tr>
<td align="center"><sub>Command palette</sub></td>
<td align="center"><sub>Markdown quick view</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/table-parquet.png"><img src="docs/light/table-parquet.png" alt="Parquet preview"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/table-excel.png"><img src="docs/light/table-excel.png" alt="Excel preview"></picture></td>
</tr>
<tr>
<td align="center"><sub>Parquet preview with row filter</sub></td>
<td align="center"><sub>Excel preview with sheet tabs</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-pdf.png"><img src="docs/light/quickview-pdf.png" alt="PDF quick view"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-notebook.png"><img src="docs/light/quickview-notebook.png" alt="Jupyter quick view"></picture></td>
</tr>
<tr>
<td align="center"><sub>Quick View: PDF</sub></td>
<td align="center"><sub>Quick View: Jupyter notebook</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/disk-usage.png"><img src="docs/light/disk-usage.png" alt="Disk usage treemap"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/transfers.png"><img src="docs/light/transfers.png" alt="Tasks"></picture></td>
</tr>
<tr>
<td align="center"><sub>Disk usage treemap</sub></td>
<td align="center"><sub>Tasks with a queue</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/batch-rename.png"><img src="docs/light/batch-rename.png" alt="Batch rename"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/drag-drop.png"><img src="docs/light/drag-drop.png" alt="Drag and drop"></picture></td>
</tr>
<tr>
<td align="center"><sub>Batch rename with live preview</sub></td>
<td align="center"><sub>Drag & drop (Shift = move)</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/terminal-tabs.png"><img src="docs/light/terminal-tabs.png" alt="Light files beside a dark terminal in one tab, with tab groups"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/terminal-profiles.png"><img src="docs/light/terminal-profiles.png" alt="Terminal profiles"></picture></td>
</tr>
<tr>
<td align="center"><sub>Light files beside a dark terminal in one tab, with tab groups</sub></td>
<td align="center"><sub>Terminal profiles in Settings</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quick-filter.png"><img src="docs/light/quick-filter.png" alt="Quick filter menu with two file categories ticked"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/disk-usage-table.png"><img src="docs/light/disk-usage-table.png" alt="Disk usage as a table"></picture></td>
</tr>
<tr>
<td align="center"><sub>Quick filter by file category</sub></td>
<td align="center"><sub>Disk usage table (depth 2)</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/bookmarks-sidebar.png"><img src="docs/light/bookmarks-sidebar.png" alt="Bookmarks in the sidebar with a folder"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/bookmarks-bar.png"><img src="docs/light/bookmarks-bar.png" alt="Horizontal bookmarks bar"></picture></td>
</tr>
<tr>
<td align="center"><sub>Bookmarks with system-folder icons and a folder</sub></td>
<td align="center"><sub>Horizontal bookmarks bar</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/terminal-themes.png"><img src="docs/light/terminal-themes.png" alt="Terminal colour themes with a live sample"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/settings-fonts.png"><img src="docs/light/settings-fonts.png" alt="Font and size settings"></picture></td>
</tr>
<tr>
<td align="center"><sub>Terminal colour themes with a live sample</sub></td>
<td align="center"><sub>Fonts and sizes for the interface and the terminal</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/settings-default-apps.png"><img src="docs/light/settings-default-apps.png" alt="Default apps per file type"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/open-with.png"><img src="docs/light/open-with.png" alt="Open with, with an option to always use the app"></picture></td>
</tr>
<tr>
<td align="center"><sub>Default apps: the system's choice, or yours per file type</sub></td>
<td align="center"><sub>Open with: always use this app, or go back to the system's</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-model.png"><img src="docs/light/quickview-model.png" alt="Quick View of a 3D model"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-font.png"><img src="docs/light/quickview-font.png" alt="Quick View of a font"></picture></td>
</tr>
<tr>
<td align="center"><sub>Quick View: 3D model you can turn</sub></td>
<td align="center"><sub>Quick View: font sample sheet</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-sqlite.png"><img src="docs/light/quickview-sqlite.png" alt="Quick View of a SQLite database"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-certificate.png"><img src="docs/light/quickview-certificate.png" alt="Quick View of a certificate"></picture></td>
</tr>
<tr>
<td align="center"><sub>Quick View: SQLite tables and views</sub></td>
<td align="center"><sub>Quick View: certificate details</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-hdf5.png"><img src="docs/light/quickview-hdf5.png" alt="Quick View of an HDF5 dataset"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/quickview-safetensors.png"><img src="docs/light/quickview-safetensors.png" alt="Quick View of a safetensors model: its tensors"></picture></td>
</tr>
<tr>
<td align="center"><sub>Quick View: HDF5 datasets as tables</sub></td>
<td align="center"><sub>Quick View: safetensors, every tensor with its shape and size</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/settings-shortcuts.png"><img src="docs/light/settings-shortcuts.png" alt="Keyboard shortcuts in Settings"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/split-numbered.png"><img src="docs/light/split-numbered.png" alt="Numbered windows in a split tab"></picture></td>
</tr>
<tr>
<td align="center"><sub>Keyboard shortcuts you can change</sub></td>
<td align="center"><sub>Numbered windows, each with its own key</sub></td>
</tr>
<tr>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/split.png"><img src="docs/light/split.png" alt="Split menu"></picture></td>
<td><picture><source media="(prefers-color-scheme: dark)" srcset="docs/dark/vietnamese.png"><img src="docs/light/vietnamese.png" alt="Vietnamese UI"></picture></td>
</tr>
<tr>
<td align="center"><sub>Split a window right, left, down or up</sub></td>
<td align="center"><sub>Vietnamese UI</sub></td>
</tr>
</table>

<a id="filter"></a>

## 🔎 Quick filter syntax

```sql
report                         -- name contains "report"
*.rs   img_??.png              -- glob patterns
.jpg .png                      -- by extension
name LIKE '%2024%'             -- % = any characters, _ = one character
ext IN ('rs', 'toml')
size > 10MB AND type = 'file'
age < 7d                       -- s, m, h, d, w, mo, y
modified >= '2024-01-01'
NOT name LIKE '.%' OR (ext = 'md' AND size < 4KB)
name REGEXP '^img_\d{4}\.jpe?g$'  -- regular expression, ignoring case
name ~ '^IMG_'                 -- ~ minds case; ~* ignores it; !~ and !~* = no match
source LIKE '%TODO%'           -- a line of the file matches, like grep
source ~ 'fn \w+\(' AND ext = 'rs'
```

**Fields:** `name` `ext` `size` `type` (`dir`/`file`) `modified` `age` `hidden` `source` (alias `content`)<br>
**Operators:** `= != <> < <= > >= LIKE NOT LIKE ILIKE GLOB IN NOT IN REGEXP RLIKE NOT REGEXP ~ ~* !~ !~* AND OR NOT ( )`. An optional leading `WHERE` is accepted.

**Regular expressions** use the [Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax) (`\d`, `\w`, `[a-z]`, `(a|b)`, `^…$`, `{2,4}`…) and work on `name`, `ext` and `source`. Put the pattern in quotes.

**`source`** reads the files like `grep`: a `LIKE` pattern or a regular expression is tried on each line, so `'%TODO%'` finds a line that contains *TODO* and `'TODO%'` one that starts with it. Folders and binary files (a NUL byte near the start) never match, `NOT LIKE` included. The files are read in the background: while you type, the folder fills in as matches are found (*Searching file contents…* in the status bar), and <kbd>↓</kbd> <kbd>Enter</kbd> searches every subfolder the same way. It works on local folders.

<a id="shortcuts"></a>

## ⌨️ Keyboard shortcuts

| Key | Action | Key | Action |
|---|---|---|---|
| <kbd>Tab</kbd> | switch panel | <kbd>F2</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | rename |
| <kbd>Enter</kbd> | open folder / archive / file | <kbd>F3</kbd> | quick-view panel on / off |
| <kbd>Backspace</kbd> | parent folder | <kbd>F4</kbd> | open: a folder here, a file with its default app |
| <kbd>Space</kbd> / <kbd>Insert</kbd> | select | <kbd>Shift</kbd>+<kbd>F4</kbd> | new blank file |
| <kbd>Shift</kbd>+<kbd>↑↓</kbd> | extend selection | <kbd>F5</kbd> / <kbd>F6</kbd> | copy / move to other panel |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> / <kbd>Ctrl</kbd>+<kbd>I</kbd> | select all / invert | <kbd>F7</kbd> | new folder |
| <kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | back / forward | <kbd>F8</kbd>, <kbd>Del</kbd> | delete (<kbd>Shift</kbd> = permanently) |
| <kbd>Shift</kbd>+<kbd>F3</kbd> | viewer window | <kbd>F9</kbd> | terminal here |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | new tab | <kbd>Alt</kbd>+<kbd>F5</kbd> / <kbd>Alt</kbd>+<kbd>F9</kbd> | compress / extract |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | reopen closed tab | <kbd>Ctrl</kbd>+<kbd>`</kbd> | new terminal tab |
| <kbd>Ctrl</kbd>+<kbd>Tab</kbd>, <kbd>Ctrl</kbd>+<kbd>PgUp</kbd> / <kbd>PgDn</kbd> | next / previous tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>PgUp</kbd> / <kbd>PgDn</kbd> | move tab |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1…8</kbd> / <kbd>9</kbd> | go to tab / last tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>\\</kbd> | last used tab |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F1…F4</kbd> | go to window 1…4 | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>]</kbd> / <kbd>[</kbd> | next / previous window |
| <kbd>Ctrl</kbd>+<kbd>W</kbd> | close window | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd> | close whole tab |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> / <kbd>E</kbd> | split window right / down | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>L</kbd> / <kbd>K</kbd> | split window left / up |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> / <kbd>Ctrl</kbd>+<kbd>L</kbd> | focus filter / path | <kbd>Alt</kbd>+<kbd>Enter</kbd> | properties |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | hidden files | <kbd>Ctrl</kbd>+<kbd>U</kbd> | swap panels |
| <kbd>Ctrl</kbd>+<kbd>D</kbd> | bookmark folder | <kbd>Ctrl</kbd>+<kbd>N</kbd> | connect to server |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | backup | <kbd>Ctrl</kbd>+<kbd>,</kbd> | settings |
| <kbd>Ctrl</kbd>+<kbd>J</kbd> | tasks | <kbd>Ctrl</kbd>+<kbd>M</kbd> | batch rename |
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | command palette | <kbd>Ctrl</kbd>+<kbd>\\</kbd> | toggle sidebar |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>H</kbd> | **checksum (create / verify)** | <kbd>F1</kbd> | all shortcuts |

> [!TIP]
> These are the Windows and Linux defaults. macOS has its own set using <kbd>⌘</kbd>, such as <kbd>⌘1…4</kbd> for windows, <kbd>⌥⌘1…9</kbd> for tabs, and <kbd>⌘\\</kbd> / <kbd>⌥⌘\\</kbd> to split right / down (<kbd>⌃⌘</kbd> + arrows for any direction), and <kbd>⌘W</kbd> / <kbd>⇧⌘W</kbd> to close a window / tab.
>
> **Change any of them in *Settings → Keyboard shortcuts*.** Windows in a split tab show their number in their title bar. <kbd>F1</kbd> lists the keys currently set.
>
> **Vim, Emacs and other terminal programs keep their keys.** While a terminal has the keyboard, the app only takes combinations they never receive (<kbd>⌘</kbd> on macOS, <kbd>Ctrl</kbd>+<kbd>Shift</kbd> elsewhere, and <kbd>Ctrl</kbd>+<kbd>Tab</kbd>). So <kbd>Ctrl</kbd>+<kbd>W</kbd> closes a file window, and in a terminal it reaches Vim; there <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>W</kbd> closes the window. This also holds for keys you set yourself.

<a id="download"></a>

## 📦 Download

**macOS on Apple silicon (M1 and later):** download `MatrixFinder-<version>-macos-arm64.dmg` from the [latest release](https://github.com/quangtiencs/matrix_finder/releases/latest), open it and drag **Matrix Finder** to **Applications**. The app is ad-hoc signed, not notarized: the first time, right-click it and choose **Open**.

**Intel Macs, Linux and Windows:** build it with the [install script](#build) below. From 0.8.0 on, releases no longer include a DMG for Intel Macs, which few people still use.

<a id="build"></a>

## 🛠️ Build from source

### Install script (macOS, Debian, Ubuntu, Kali, Fedora)

`scripts/install.sh` builds Matrix Finder and installs it. It installs the packages and Rust it needs (it asks first), builds, and installs:

- **macOS**: `Matrix Finder.app` in `/Applications` (or `~/Applications`) and a `matrix_finder` command.
- **Linux**: the `matrix_finder` command plus a menu entry and icons in `~/.local` (`--system` installs for all users under `/usr/local`). Debian-based systems (Debian, Ubuntu, Kali, Mint, Pop!_OS, Raspberry Pi OS…) install packages with `apt`, Fedora-based ones with `dnf`.

Install the newest release:

```bash
curl -fsSL https://raw.githubusercontent.com/quangtiencs/matrix_finder/main/scripts/install.sh | bash
```

Options go after `bash -s --` (e.g. `| bash -s -- --version 0.3.0`). `| sh` and `sh install.sh` work too: where `sh` is not bash (dash on Debian, Ubuntu and Kali), the script runs itself again in bash. From a checkout of this repository:

```bash
scripts/install.sh                     # build and install this checkout
scripts/install.sh --version 0.3.0     # a given release (--ref for a branch or commit)
scripts/install.sh --uninstall         # remove it again (settings are kept)
```

On macOS the script first checks that the compiler can link with the macOS SDK. If the Command Line Tools are half-updated (the build would fail with `tapi error: malformed file … unknown architecture`), it builds with an older SDK that works, or says how to repair the tools.

Run it again to upgrade: the new version replaces the old one in place, and files the old one had but the new one does not are removed. Source downloads and build output are kept in `~/.cache/matrix_finder` (`~/Library/Caches/matrix_finder` on macOS), so upgrades build faster. `--help` lists every option.

### By hand

Requires **Rust 1.85+** and a C/C++ compiler (the RAR reader and libssh2 are written in C/C++).

```bash
cargo build --release
```

The binary is written to `target/release/matrix_finder` (`.exe` on Windows).

| Platform | Setup |
|---|---|
| 🐧 **Linux** | Install a C/C++ compiler, pkg-config, the OpenSSL headers and the X11/Wayland libraries egui needs. Debian/Ubuntu: `sudo apt install build-essential pkg-config libssl-dev libxkbcommon-x11-0 libgl1 libegl1`. Fedora: `sudo dnf install gcc gcc-c++ make pkgconf-pkg-config openssl-devel libxkbcommon-x11 mesa-libGL mesa-libEGL` |
| 🍎 **macOS** | Install the Xcode command line tools. For SFTP, use Homebrew OpenSSL (`brew install openssl@3`) or build with `--features vendored-openssl` |
| 🪟 **Windows** | Install the MSVC build tools. SFTP uses the built-in Windows crypto (WinCNG), so OpenSSL is not needed |

### Starting from a terminal

```bash
matrix_finder              # the current folder
matrix_finder ~/src        # a folder; a file opens its folder with the cursor on it
matrix_finder backup.zip   # an archive, or sftp://user@host/path
```

If Matrix Finder is already running, the folder opens in a **new tab of the running window** and the command returns right away. To start a separate window instead, change *Settings → Files & layout → Window → When Matrix Finder is started again*, or pass `--new-window` for one launch (`--new-tab` does the opposite).
| ✂️ **No SFTP** | `cargo build --release --no-default-features` (no OpenSSL or libssh2 at all) |

<details>
<summary><b>🍎 macOS app and DMG</b></summary>

```bash
make dmg            # or: scripts/macos/build-dmg.sh
```

This builds `target/macos/Matrix Finder.app` and `target/macos/MatrixFinder-<version>-macos-<arch>.dmg`. Open the DMG and drag **Matrix Finder** onto **Applications**.

- `make dmg-universal` builds one binary for both Apple silicon and Intel (releases ship only the Apple silicon DMG since 0.8.0, so this is the way to a DMG for an Intel Mac), and `make app` builds only the `.app`.
- `--sign "Developer ID Application: …"` applies a real signature. The default is an ad-hoc signature, so macOS asks for confirmation the first time you open the app (right-click → Open).
- `--no-layout` skips the Finder window styling, and `BUNDLE_ID=…` changes the bundle identifier.
- After editing the SVGs in `assets/logo`, run `make icons` to regenerate the PNGs, the `.icns` file and the DMG background.
- `make screenshots` re-renders the screenshots headlessly from a generated demo folder: every scene to `docs/light` and `docs/dark`, plus `docs/showcase.png` (`SHOTS=panels,palette` renders only some).

</details>

<details>
<summary><b>🧪 Tests</b></summary>

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
- **checksum digests, list parsing and verification**
- the NumPy, safetensors, ONNX and R data readers, with cut-short and damaged files
- translation completeness

Live FTP/SFTP round-trips run against your own servers:

```bash
MATRIX_FINDER_TEST_FTP=127.0.0.1:21:user:pass cargo test live_ftp -- --ignored
MATRIX_FINDER_TEST_SFTP=host:22:user:/path/to/private_key cargo test live_sftp -- --ignored
```

</details>

<a id="settings"></a>

## 🌍 Languages, settings and backups

- **Updates**: *Check for updates…* (in the app menu, *Help*, the About window or the command palette) asks GitHub for the latest release. If there is a newer one, a window shows its version, date and release notes, with **Download** (the Apple silicon DMG; on an Intel Mac, build it with the install script instead) and **Open release page**, both opened in your browser. Installing is up to you, as before. *Skip this version* keeps the check at start from offering it again. To check at start (at most once a day), turn on *Settings → Files & layout → Check for updates when Matrix Finder starts*; it is off by default (see [Privacy](#privacy)).
- **Settings** are stored in `<config dir>/matrix_finder/settings.json`: `~/.config` on Linux, `~/Library/Application Support` on macOS and `%APPDATA%` on Windows. *File → Export settings…* writes a copy **without passwords**.
- **Languages** live in `<config dir>/matrix_finder/lang/<code>.json`. *Settings → Export translation template…* writes every key for you, and missing keys fall back to English.
  ```json
  { "code": "fr", "name": "Français", "strings": { "Copy": "Copier", "Delete": "Supprimer" } }
  ```
- **Passwords** for saved connections are stored only if you tick *Also save the password*, and they are stored in plain text. Prefer SSH keys or the SSH agent.

<a id="privacy"></a>

## 🔒 Privacy

Matrix Finder collects no data and sends no telemetry: no analytics, no crash reports. It goes online only when you ask it to: by connecting to an FTP or SFTP server, clicking a link (which opens in your browser), or checking for updates. It also listens on a local port so a second launch can open in the running window. That port is on `localhost` only and is never reachable from the network.

**Update checks** happen only when you choose *Check for updates…*, or at start (at most once a day) if you turn on *Settings → Files & layout → Check for updates when Matrix Finder starts*, which is **off by default**. The check asks GitHub's public API for the latest release of this project, using `curl` (or `wget`), with only the app's name and version in the request. Nothing about you or your files is sent, and nothing is downloaded or installed by the app.

<a id="layout"></a>

## 🧭 Project layout

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
├── rename.rs      batch rename rules, validation, safe apply/undo and its tab
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

<a id="license"></a>

## 📄 License

The code is under the **MIT** license. The bundled fonts are under the SIL Open Font License 1.1 (`assets/fonts/OFL.txt`). The AI coding agent logos come from [LobeHub Icons](https://github.com/lobehub/lobe-icons) (MIT, `assets/agents/LICENSE.txt`); they are trademarks of their owners and only show which tool the AI button starts.

<div align="center">
<br>
<img src="assets/logo/matrix_finder-64.png" width="40" alt="">
<br>
<sub>Made with 🦀 Rust and egui · An independent project, not affiliated with <i>The Matrix</i> films or their owners.</sub>
</div>
