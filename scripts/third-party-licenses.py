#!/usr/bin/env python3
"""Write THIRD-PARTY-LICENSES.md: the licenses of everything built into Matrix Finder.

    scripts/third-party-licenses.py           # rewrite THIRD-PARTY-LICENSES.md
    scripts/third-party-licenses.py --check   # fail if it is out of date (CI)

Lists every Rust crate compiled into the program (normal dependencies, all targets),
the C libraries the -sys crates build into it, and the bundled fonts and icons, with
their license texts. The texts come from the crates' own files (`cargo metadata`, so the
crates must be downloaded: `cargo fetch`) and from assets/. Needs Python 3.8+ and cargo.
"""
import hashlib
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD-PARTY-LICENSES.md"
LICENSE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE|UFL|OFL)([-._].*)?$", re.I)

# C and C++ code that -sys crates compile into the program: (crate, title, what and
# where, license, license files inside the crate).
C_LIBRARIES = [
    ("unrar_sys", "UnRAR", "Reads RAR archives. In every build.", "UnRAR license (freeware)",
     ["vendor/unrar/license.txt"]),
    ("libssh2-sys", "libssh2", "SFTP. Built in on macOS and in the Linux packages.", "BSD-3-Clause",
     ["libssh2/COPYING"]),
    ("openssl-src", "OpenSSL", "Encryption for SFTP. Built into the macOS app; the Linux packages use the system's libssl3.",
     "Apache-2.0", ["openssl/LICENSE.txt"]),
    ("libsqlite3-sys", "SQLite", "Previews of SQLite databases. In every build.", "Public domain",
     []),
    ("zstd-sys", "Zstandard", "Zstandard archives and Parquet/Arrow files. In every build.", "BSD-3-Clause",
     ["zstd/LICENSE"]),
    ("bzip2-sys", "bzip2", "bzip2 archives. The macOS app uses the system's libbz2.", "bzip2-1.0.6",
     ["bzip2-1.0.8/LICENSE"]),
    ("lzma-sys", "liblzma (XZ Utils)", "xz archives. The macOS app uses the system's liblzma.", "Public domain (liblzma)",
     ["xz-5.2/COPYING"]),
    ("libz-sys", "zlib", "Deflate (zip, gzip). The macOS app uses the system's libz.", "Zlib",
     ["src/zlib/LICENSE"]),
]
# Where each library's own version is written: (file in the crate, patterns whose first
# groups are joined with dots).
VERSIONS = {
    "unrar_sys": ("vendor/unrar/version.hpp", [r"define RARVER_MAJOR\s+(\d+)", r"define RARVER_MINOR\s+(\d+)"]),
    "libssh2-sys": ("libssh2/include/libssh2.h", [r'define LIBSSH2_VERSION\s+"([^"]+)"']),
    "openssl-src": ("openssl/VERSION.dat", [r"MAJOR=(\d+)", r"MINOR=(\d+)", r"PATCH=(\d+)"]),
    "libsqlite3-sys": ("sqlite3/sqlite3.h", [r'define SQLITE_VERSION\s+"([^"]+)"']),
    "zstd-sys": ("zstd/lib/zstd.h", [r"define ZSTD_VERSION_MAJOR\s+(\d+)", r"define ZSTD_VERSION_MINOR\s+(\d+)", r"define ZSTD_VERSION_RELEASE\s+(\d+)"]),
    "bzip2-sys": ("bzip2-1.0.8/bzlib_private.h", [r'define BZ_VERSION\s+"([0-9.]+)']),
    "lzma-sys": ("xz-5.2/src/liblzma/api/lzma/version.h", [r"define LZMA_VERSION_MAJOR (\d+)", r"define LZMA_VERSION_MINOR (\d+)", r"define LZMA_VERSION_PATCH (\d+)"]),
    "libz-sys": ("src/zlib/zlib.h", [r'define ZLIB_VERSION "([^"]+)"']),
}

# Crates whose files carry licenses for what they bundle besides code: (crate, what,
# license files inside the crate).
CRATE_ASSETS = [
    ("epaint_default_fonts", "egui's default fonts (Hack, Ubuntu Light, Noto Emoji, emoji-icon-font)",
     ["fonts/Hack-Regular.txt", "fonts/UFL.txt", "fonts/OFL.txt", "fonts/emoji-icon-font-mit-license.txt"]),
]

# A copyright line of a license file ("Copyright (c) 2016 Someone", "(c) 2020 …"), but
# not one of the license's own sentences that start with the word.
COPYRIGHT_LINE = re.compile(r"^\s*(copyright\s*(\(c\)|©|\d{4})|\(c\)\s*\d{4}|©)", re.I)

# For a crate offered under several licenses, the license files kept, by preference.
PREFERRED = ["MIT", "APACHE", "ZLIB", "BSD", "UNLICENSE"]

SQLITE_NOTE = """The author disclaims copyright to this source code. In place of a legal notice, here
is a blessing:

    May you do good and not evil.
    May you find forgiveness for yourself and forgive others.
    May you share freely, never taking more than you give.

(SQLite is in the public domain: https://sqlite.org/copyright.html)"""

# Fonts and icons in assets/, built into the program: (what, license, files).
ASSETS = [
    ("Source Sans 3, Source Serif 4, Source Code Pro (Adobe)", "OFL-1.1", ["assets/fonts/OFL.txt"]),
    ("JetBrains Mono NL", "OFL-1.1", ["assets/fonts/OFL-JetBrainsMono.txt"]),
    ("Noto Sans Symbols 2", "OFL-1.1", ["assets/fonts/OFL-NotoSansSymbols2.txt"]),
    ("DejaVu Sans Mono", "Bitstream Vera and Arev fonts licenses, public domain changes", ["assets/fonts/LICENSE-DejaVu.txt"]),
    ("Symbols Nerd Font Mono", "MIT (Nerd Fonts); glyphs under their own licenses, see below", ["assets/fonts/LICENSE-NerdFontsSymbols.txt"]),
    ("AI tool logos (LobeHub Icons)", "MIT; the logos are trademarks of their owners", ["assets/agents/LICENSE.txt"]),
    ("Material Icon Theme (file and folder icons)", "MIT; logos among them are trademarks of their owners", ["assets/icons/material/LICENSE"]),
    ("Fluent UI System Icons (Microsoft)", "MIT", ["assets/icons/fluent/LICENSE"]),
]
NERD_FONT_GLYPHS = """The glyphs in Symbols Nerd Font come from these icon sets (from Nerd Fonts'
license-audit.md, https://github.com/ryanoasis/nerd-fonts):

| Icon set | License |
|---|---|
| Codicons | CC BY 4.0 |
| Devicons | MIT |
| Font Awesome | CC BY 4.0 |
| Font Awesome Extension | MIT |
| Font Logos | Unlicense |
| IEC Power Symbols | MIT |
| Material Design Icons | Apache 2.0 |
| Seti-UI (modified) | MIT |
| Octicons | MIT |
| Pomicons | SIL OFL 1.1 |
| Powerline Extra Symbols | MIT |
| Powerline Symbols | Free license |
| Weather Icons | SIL OFL 1.1 |"""


def cargo(*args):
    return subprocess.run(["cargo", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def shipped():
    """(name, version) of every crate compiled into the program, on any target."""
    out = cargo("tree", "--locked", "-e", "normal", "--target", "all", "--prefix", "none", "-f", "{p}")
    seen = set()
    for line in out.splitlines():
        m = re.match(r"^(\S+) v(\S+)", line)
        if m and m.group(1) != "matrix_finder":
            seen.add((m.group(1), m.group(2)))
    return seen


def read(path):
    text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
    return "\n".join(l.rstrip() for l in text.strip("\n").split("\n"))


def upstream_version(crate, base, fallback):
    spec = VERSIONS.get(crate)
    if not spec:
        return fallback
    try:
        text = (base / spec[0]).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return fallback
    parts = [m.group(1) for m in (re.search(r, text) for r in spec[1]) if m]
    return ".".join(parts) if len(parts) == len(spec[1]) else fallback


def license_files(base, expression):
    """The license files of the crate in `base`: for one offered under a choice of
    licenses ("MIT OR Apache-2.0"), only those of the preferred one when they are told
    apart by name (LICENSE-MIT, LICENSE-APACHE)."""
    files = sorted(f for f in base.iterdir() if f.is_file() and LICENSE_FILE.match(f.name))
    if " OR " in expression or "/" in expression:
        for pref in PREFERRED:
            named = [f for f in files if pref in f.name.upper()]
            if named and pref in expression.upper().replace("APACHE-2.0", "APACHE"):
                generic = [f for f in files if not any(p in f.name.upper() for p in PREFERRED)]
                return named + generic
    return files


def block(text):
    fence = "```"
    while fence in text:
        fence += "`"
    return f"{fence}text\n{text}\n{fence}\n"


def render():
    meta = json.loads(cargo("metadata", "--locked", "--all-features", "--format-version", "1"))
    packages = {(p["name"], p["version"]): p for p in meta["packages"]}
    by_name = {}
    for p in meta["packages"]:
        by_name.setdefault(p["name"], p)
    crates = sorted(shipped(), key=lambda nv: (nv[0].lower(), nv[1]))

    out = []
    w = out.append
    w("# Third-party licenses\n")
    w("Matrix Finder is under the MIT license (see `LICENSE`). The program is built from the code, fonts and")
    w("icons below, each under its own license. This file is generated by `scripts/third-party-licenses.py`;")
    w("do not edit it by hand.\n")

    w("## Summary\n")
    w("| Component | Version | License | Used for |")
    w("|---|---|---|---|")
    for crate, title, what, lic, _ in C_LIBRARIES:
        p = by_name[crate]
        version = upstream_version(crate, pathlib.Path(p["manifest_path"]).parent, "")
        w(f"| {title} (C, built by the `{crate}` crate) | {version} | {lic} | {what} |")
    for what, lic, _ in ASSETS:
        w(f"| {what} | | {lic} | Built into the program |")
    for crate, what, _ in CRATE_ASSETS:
        w(f"| {what} | | {by_name[crate].get('license')} | Built into the program by the `{crate}` crate |")
    w(f"| {len(crates)} Rust crates | | see below | |\n")
    w("UnRAR's license allows its use to read RAR archives, but not to write a RAR-compatible archiver or")
    w("to re-create the RAR compression algorithm. Matrix Finder only reads RAR archives.\n")

    w("## Rust crates\n")
    w("| Crate | Version | License |")
    w("|---|---|---|")
    for name, version in crates:
        lic = packages[(name, version)].get("license") or "see its license file"
        w(f"| {name} | {version} | {lic} |")
    w("")

    w("## License texts\n")
    w("### C libraries\n")
    for crate, title, _, lic, files in C_LIBRARIES:
        p = by_name[crate]
        base = pathlib.Path(p["manifest_path"]).parent
        w(f"#### {title}\n")
        if not files:
            w(block(SQLITE_NOTE))
        for f in files:
            w(block(read(base / f)))
    w("### Fonts and icons\n")
    for what, _, files in ASSETS:
        w(f"#### {what}\n")
        for f in files:
            w(block(read(ROOT / f)))
    w(NERD_FONT_GLYPHS + "\n")
    for crate, what, files in CRATE_ASSETS:
        base = pathlib.Path(by_name[crate]["manifest_path"]).parent
        w(f"#### {what}\n")
        for f in files:
            w(block(read(base / f)))

    # Crates: each distinct text once, with the crates that carry it.
    texts = {}
    missing = []
    for name, version in crates:
        p = packages[(name, version)]
        base = pathlib.Path(p["manifest_path"]).parent
        files = license_files(base, p.get("license") or "")
        if not files:
            authors = ", ".join(re.sub(r"\s*<[^>]*>", "", a) for a in p.get("authors") or []) or "its authors"
            missing.append(f"| {name} | {version} | {p.get('license') or 'not given'} | {authors} |")
            continue
        for f in files:
            # Texts that differ only in their copyright lines are shown once, with all
            # of those lines.
            lines = read(f).split("\n")
            notices = [l.strip() for l in lines if COPYRIGHT_LINE.match(l)]
            body = "\n".join(l for l in lines if not COPYRIGHT_LINE.match(l)).strip("\n")
            key = hashlib.sha256(re.sub(r"\s+", " ", body).encode()).hexdigest()
            group = texts.setdefault(key, [body, [], []])
            group[1].append(f"{name} {version}")
            group[2].extend(n for n in notices if n not in group[2])
    w("### Rust crates\n")
    w("Each license text once, after the crates whose packages include it and their copyright notices.\n")
    for body, users, notices in sorted(texts.values(), key=lambda g: g[1][0].lower()):
        w(f"Used by: {', '.join(sorted(set(users), key=str.lower))}\n")
        if notices:
            w(block("\n".join(notices)))
        w(block(body))
    if missing:
        w("These crates ship no license file. Each is under the license its package declares, whose")
        w("standard text is above; the copyright is its authors'.\n")
        w("| Crate | Version | License | Copyright |")
        w("|---|---|---|---|")
        out.extend(missing)
        w("")
    return "\n".join(out).rstrip("\n") + "\n"


def main():
    text = render()
    if "--check" in sys.argv[1:]:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            sys.exit("THIRD-PARTY-LICENSES.md is out of date: run scripts/third-party-licenses.py")
        return
    OUT.write_text(text, encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {len(text) // 1024} KB")


if __name__ == "__main__":
    main()
