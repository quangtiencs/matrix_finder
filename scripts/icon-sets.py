#!/usr/bin/env python3
"""Copy the bundled icon sets into assets/icons/ and write their Rust tables.

The icons come from two npm packages, both MIT:
  - material-icon-theme (Material Icon Theme, https://github.com/material-extensions/vscode-material-icon-theme)
  - @fluentui/svg-icons (Fluent UI System Icons, https://github.com/microsoft/fluentui-system-icons)

Download and unpack them first (each package's files are in its `package/` folder):

    curl -fsSLO https://registry.npmjs.org/material-icon-theme/-/material-icon-theme-5.40.0.tgz
    curl -fsSLO https://registry.npmjs.org/@fluentui/svg-icons/-/svg-icons-1.1.345.tgz
    mkdir material fluent && tar -xzf material-icon-theme-*.tgz -C material && tar -xzf svg-icons-*.tgz -C fluent
    python3 scripts/icon-sets.py material/package fluent/package

Writes assets/icons/{material,fluent}/*.svg and src/style/iconsets/{material,fluent}.rs.
Fluent's LICENSE is not in its package: assets/icons/fluent/LICENSE is kept as it is.
"""
import json
import os
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Languages VS Code finds by extension, where Material Icon Theme names the language
# rather than the extension: (language id, extensions).
LANGUAGE_EXTS = [
    ("javascript", ["js", "mjs", "cjs"]),
    ("typescript", ["ts", "mts", "cts"]),
    ("javascriptreact", ["jsx"]),
    ("typescriptreact", ["tsx"]),
    ("python", ["py", "pyw", "pyi"]),
    ("html", ["html", "htm", "xhtml"]),
    ("css", ["css"]),
    ("scss", ["scss"]),
    ("json", ["json"]),
    ("jsonc", ["jsonc"]),
    ("markdown", ["md", "markdown"]),
    ("yaml", ["yml", "yaml"]),
    ("xml", ["xml", "xsd", "plist"]),
    ("shellscript", ["sh", "bash", "zsh", "fish", "ksh"]),
    ("bat", ["bat", "cmd"]),
    ("powershell", ["ps1", "psm1", "psd1"]),
    ("c", ["c"]),
    ("cpp", ["cpp", "cc", "cxx", "hpp", "hh", "hxx"]),
    ("csharp", ["cs"]),
    ("go", ["go"]),
    ("java", ["java"]),
    ("php", ["php"]),
    ("ruby", ["rb"]),
    ("rust", ["rs"]),
    ("swift", ["swift"]),
    ("sql", ["sql"]),
    ("lua", ["lua"]),
    ("perl", ["pl", "pm"]),
    ("r", ["r"]),
    ("dart", ["dart"]),
    ("scala", ["scala", "sc"]),
    ("haskell", ["hs"]),
    ("clojure", ["clj", "cljs", "edn"]),
    ("elixir", ["ex", "exs"]),
    ("erlang", ["erl"]),
    ("julia", ["jl"]),
    ("latex", ["tex"]),
    ("vue", ["vue"]),
    ("svelte", ["svelte"]),
    ("toml", ["toml"]),
    ("ini", ["ini", "cfg", "conf"]),
    ("log", ["log"]),
    ("diff", ["diff", "patch"]),
    ("plaintext", ["txt"]),
    ("csv", ["csv"]),
    ("tsv", ["tsv"]),
    ("pdf", ["pdf"]),
    ("objective-c", ["m"]),
    ("objective-cpp", ["mm"]),
    ("fsharp", ["fs", "fsx"]),
    ("groovy", ["groovy", "gradle"]),
    ("graphql", ["graphql", "gql"]),
    ("proto", ["proto"]),
    ("nix", ["nix"]),
    ("terraform", ["tf"]),
    ("matlab", ["mat"]),
    ("jupyter", ["ipynb"]),
]

# Fluent icons used, by the name the code asks for: (name, file stem without size and
# style, style). The largest size there is is taken.
FLUENT = [
    ("folder", "folder", "filled"),
    ("folder_link", "folder_link", "regular"),
    ("document", "document", "regular"),
    ("text", "document_text", "regular"),
    ("word", "document_word", "regular"),
    ("table", "document_table", "regular"),
    ("csv", "document_csv", "regular"),
    ("slides", "slide_text", "regular"),
    ("pdf", "document_pdf", "regular"),
    ("image", "image", "regular"),
    ("video", "video_clip", "regular"),
    ("audio", "music_note_2", "regular"),
    ("archive", "folder_zip", "regular"),
    ("code", "document_code", "regular"),
    ("javascript", "document_javascript", "regular"),
    ("css", "document_css", "regular"),
    ("java", "document_java", "regular"),
    ("yml", "document_yml", "regular"),
    ("console", "window_console", "regular"),
    ("app", "app_generic", "regular"),
    ("disk", "hard_drive", "regular"),
    ("font", "text_font", "regular"),
    ("data", "document_data", "regular"),
    ("database", "database", "regular"),
    ("model", "brain_circuit", "regular"),
    ("key", "key", "regular"),
    ("lock", "lock_closed", "regular"),
    ("settings", "settings", "regular"),
    ("book", "book", "regular"),
    ("notebook", "notebook", "regular"),
]


def rust_str(s):
    return json.dumps(s, ensure_ascii=False)


def material(pkg):
    m = json.load(open(os.path.join(pkg, "dist", "material-icons.json")))
    defs = m["iconDefinitions"]
    ext = {k.lower(): v for k, v in m["fileExtensions"].items()}
    for lang, exts in LANGUAGE_EXTS:
        icon = m["languageIds"].get(lang)
        for e in exts:
            if icon and e not in ext:
                ext[e] = icon
    names = {k.lower(): v for k, v in m["fileNames"].items()}
    folders = {k.lower(): v for k, v in m["folderNames"].items()}
    # The light theme's icon for a dark one, where it has its own.
    light = {}
    for table, base in (("fileExtensions", ext), ("fileNames", names), ("folderNames", folders)):
        for k, v in m["light"].get(table, {}).items():
            dark = base.get(k.lower())
            if dark and dark != v:
                light[dark] = v
    used = set(ext.values()) | set(names.values()) | set(folders.values()) | set(light.values()) | {m["file"], m["folder"]}
    out = os.path.join(ROOT, "assets", "icons", "material")
    for f in os.listdir(out) if os.path.isdir(out) else []:
        if f.endswith(".svg"):
            os.remove(os.path.join(out, f))
    os.makedirs(out, exist_ok=True)
    shutil.copy(os.path.join(pkg, "LICENSE"), os.path.join(out, "LICENSE"))
    for i in sorted(used):
        src = os.path.normpath(os.path.join(pkg, "dist", defs[i]["iconPath"]))
        shutil.copy(src, os.path.join(out, i + ".svg"))
    version = json.load(open(os.path.join(pkg, "package.json")))["version"]
    lines = [
        "// Generated by scripts/icon-sets.py from material-icon-theme " + version + " (MIT, see",
        "// assets/icons/material/LICENSE). Do not edit: run the script again.",
        "#![cfg_attr(rustfmt, rustfmt_skip)]",
        "",
        "pub const FILE: &str = " + rust_str(m["file"]) + ";",
        "pub const FOLDER: &str = " + rust_str(m["folder"]) + ";",
        "",
        "/// The icons by name, sorted.",
        "pub static ICONS: &[(&str, &[u8])] = &[",
    ]
    for i in sorted(used):
        lines.append("    (" + rust_str(i) + ", include_bytes!(\"../../../assets/icons/material/" + i + ".svg\")),")
    lines.append("];")
    for name, table, doc in (
        ("EXTENSIONS", ext, "File extensions (lower case, without the dot; some have several parts)"),
        ("FILE_NAMES", names, "Whole file names (lower case)"),
        ("FOLDER_NAMES", folders, "Folder names (lower case)"),
        ("LIGHT", light, "The icon for light themes, where a dark one has its own"),
    ):
        lines += ["", "/// " + doc + " → icon, sorted.", "pub static " + name + ": &[(&str, &str)] = &["]
        for k in sorted(table):
            lines.append("    (" + rust_str(k) + ", " + rust_str(table[k]) + "),")
        lines.append("];")
    write(os.path.join(ROOT, "src", "style", "iconsets", "material.rs"), "\n".join(lines) + "\n")
    size = sum(os.path.getsize(os.path.join(out, i + ".svg")) for i in used)
    print(f"material: {len(used)} icons, {size // 1024} KB; {len(ext)} extensions, {len(names)} file names, {len(folders)} folders")


def fluent(pkg):
    icons = os.path.join(pkg, "icons")
    files = set(os.listdir(icons))
    out = os.path.join(ROOT, "assets", "icons", "fluent")
    for f in os.listdir(out) if os.path.isdir(out) else []:
        if f.endswith(".svg"):
            os.remove(os.path.join(out, f))
    os.makedirs(out, exist_ok=True)
    version = json.load(open(os.path.join(pkg, "package.json")))["version"]
    lines = [
        "// Generated by scripts/icon-sets.py from @fluentui/svg-icons " + version + " (MIT, see",
        "// assets/icons/fluent/LICENSE). Do not edit: run the script again.",
        "#![cfg_attr(rustfmt, rustfmt_skip)]",
        "",
        "/// The icons by name, sorted: one-colour shapes, tinted when drawn.",
        "pub static ICONS: &[(&str, &[u8])] = &[",
    ]
    total = 0
    for name, stem, style in sorted(FLUENT):
        sizes = [int(f[len(stem) + 1 : -len(style) - 5]) for f in files if f.startswith(stem + "_") and f.endswith("_" + style + ".svg") and f[len(stem) + 1 : -len(style) - 5].isdigit()]
        if not sizes:
            sys.exit(f"fluent: no {stem}_*_{style}.svg")
        src = f"{stem}_{max(sizes)}_{style}.svg"
        shutil.copy(os.path.join(icons, src), os.path.join(out, name + ".svg"))
        total += os.path.getsize(os.path.join(out, name + ".svg"))
        lines.append("    (" + rust_str(name) + ", include_bytes!(\"../../../assets/icons/fluent/" + name + ".svg\")),")
    lines.append("];")
    write(os.path.join(ROOT, "src", "style", "iconsets", "fluent.rs"), "\n".join(lines) + "\n")
    print(f"fluent: {len(FLUENT)} icons, {total // 1024} KB")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    material(sys.argv[1])
    fluent(sys.argv[2])
