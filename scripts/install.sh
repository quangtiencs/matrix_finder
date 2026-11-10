#!/usr/bin/env bash
# Build Matrix Finder from source and install it (macOS, Ubuntu/Debian, Fedora).
#
#   scripts/install.sh                  # build this checkout and install it
#   scripts/install.sh --latest         # build the newest release from GitHub
#   scripts/install.sh --version 0.3.0  # a given release (also: --ref <branch/tag/commit>)
#   scripts/install.sh --uninstall      # remove what this script installed
#
#   curl -fsSL https://raw.githubusercontent.com/quangtiencs/matrix_finder/main/scripts/install.sh | bash
#
# Options:
#   --system        Linux: install for all users under /usr/local (asks for sudo)
#   --prefix DIR    Linux: install under DIR (default ~/.local); macOS: put the
#                   matrix_finder command in DIR/bin
#   --app-dir DIR   macOS: folder for the app (default /Applications, or
#                   ~/Applications when /Applications is not writable)
#   --no-deps       do not install system packages or Rust; only check
#   -y, --yes       install missing packages and Rust without asking
#
# Running it again upgrades (or downgrades) in place: files of the earlier install
# that the new version no longer has are removed, and settings are kept. A running
# Matrix Finder is left alone; quit and reopen it to use the new version.
#
# Environment: MATRIX_FINDER_REPO (git URL to build from), FEATURES (cargo
# features; macOS default "--features vendored-openssl").
#
# Written for bash 3.2 (macOS's /bin/bash): no associative arrays, no mapfile.
# shellcheck disable=SC1091  # ~/.cargo/env and /etc/os-release exist at run time
set -euo pipefail

REPO=${MATRIX_FINDER_REPO:-https://github.com/quangtiencs/matrix_finder.git}
APP_ID=org.matrixfinder.MatrixFinder
APP_NAME="Matrix Finder"

usage() {
    cat <<'EOF'
Build Matrix Finder from source and install it (macOS, Ubuntu/Debian, Fedora).

Usage: install.sh [options]

  (no option)         build the checkout this script is in, or the newest
                      release when run on its own (e.g. through curl | bash)
  --latest            build the newest release from GitHub
  --version X.Y.Z     build that release
  --ref REF           build a branch, tag or commit
  --uninstall         remove what this script installed (settings are kept)

  --system            Linux: install for all users under /usr/local (sudo)
  --prefix DIR        Linux: install under DIR (default ~/.local);
                      macOS: put the matrix_finder command in DIR/bin
  --app-dir DIR       macOS: folder for the app (default /Applications,
                      or ~/Applications when that is not writable)
  --no-deps           do not install system packages or Rust, only check
  -y, --yes           install missing packages and Rust without asking
  -h, --help          show this help

Running it again upgrades or downgrades in place and removes files the new
version no longer has. Environment: MATRIX_FINDER_REPO (git URL), FEATURES
(cargo features; macOS default "--features vendored-openssl").
EOF
}

# Colours only on a terminal.
if [ -t 1 ]; then C_BOLD=$'\033[1m' C_WARN=$'\033[33m' C_ERR=$'\033[31m' C_OFF=$'\033[0m'
else C_BOLD="" C_WARN="" C_ERR="" C_OFF=""; fi
say() { printf '%s==>%s %s\n' "$C_BOLD" "$C_OFF" "$*"; }
warn() { printf '%swarning:%s %s\n' "$C_WARN" "$C_OFF" "$*" >&2; }
die() { printf '%serror:%s %s\n' "$C_ERR" "$C_OFF" "$*" >&2; exit 1; }

# ---------------------------------------------------------------- options

mode=install ref="" want_latest=0 prefix="" system=0 app_dir="" deps=1 yes=0
while [ $# -gt 0 ]; do
    case "$1" in
        --latest) want_latest=1 ;;
        --version) [ $# -ge 2 ] || die "--version needs a version"; ref="v${2#v}"; shift ;;
        --ref) [ $# -ge 2 ] || die "--ref needs a branch, tag or commit"; ref="$2"; shift ;;
        --system) system=1 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a folder"; prefix="$2"; shift ;;
        --app-dir) [ $# -ge 2 ] || die "--app-dir needs a folder"; app_dir="$2"; shift ;;
        --no-deps) deps=0 ;;
        --uninstall) mode=uninstall ;;
        -y|--yes) yes=1 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
    shift
done

case "$(uname -s)" in
    Darwin) os=mac ;;
    Linux) os=linux ;;
    *) die "this script supports macOS and Linux; on Windows build with: cargo build --release" ;;
esac
[ "$system" = 1 ] && [ "$os" = mac ] && die "--system is for Linux; on macOS use --app-dir"
[ "$system" = 1 ] && [ -n "$prefix" ] && die "use either --system or --prefix"
[ "$system" = 1 ] && prefix=/usr/local
# Absolute paths: the build runs in the source folder.
absolute() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
[ -z "$prefix" ] || prefix=$(absolute "$prefix")
[ -z "$app_dir" ] || app_dir=$(absolute "$app_dir")

# ---------------------------------------------------------------- helpers

# Yes/no question; the answer comes from the terminal even when the script is piped
# into bash. Without a terminal (and without --yes) the answer is no.
ask() {
    [ "$yes" = 1 ] && return 0
    [ -r /dev/tty ] || return 1
    local answer
    printf '%s [Y/n] ' "$1" >/dev/tty
    read -r answer </dev/tty || return 1
    case "$answer" in ""|y|Y|yes|Yes) return 0 ;; *) return 1 ;; esac
}

# Compare dotted versions: prints -1, 0 or 1 (1.85 < 1.85.1 < 1.86; "v" and suffixes ignored).
ver_cmp() {
    local a=${1#v} b=${2#v} x y
    a=${a%%[-+]*}; b=${b%%[-+]*}
    while [ -n "$a" ] || [ -n "$b" ]; do
        x=${a%%.*}; if [ "$x" = "$a" ]; then a=""; else a=${a#*.}; fi
        y=${b%%.*}; if [ "$y" = "$b" ]; then b=""; else b=${b#*.}; fi
        x=${x%%[!0-9]*}; y=${y%%[!0-9]*}
        x=$((10#${x:-0})); y=$((10#${y:-0}))
        if [ "$x" -lt "$y" ]; then echo -1; return; fi
        if [ "$x" -gt "$y" ]; then echo 1; return; fi
    done
    echo 0
}

# "sudo" when DIR (or its nearest existing parent) is not writable by us.
sudo_for() {
    local d=$1
    while [ ! -e "$d" ] && [ "$d" != / ]; do d=$(dirname "$d"); done
    if [ -w "$d" ] || [ "$(id -u)" = 0 ]; then echo ""; else
        command -v sudo >/dev/null || die "$d is not writable and sudo is missing"
        echo sudo
    fi
}

# Value of `key = "value"` in the [package] table of a Cargo.toml.
cargo_field() {
    awk -v k="$2" '/^\[/{p=($0=="[package]")} p && $1==k {sub(/^[^=]*= *"/,""); sub(/".*$/,""); print; exit}' "$1"
}

on_path() { case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac; }

if [ "$os" = mac ]; then
    state_dir="$HOME/Library/Application Support/matrix_finder"
    cache_dir="$HOME/Library/Caches/matrix_finder"
else
    state_dir="${prefix:-$HOME/.local}/share/matrix_finder"
    cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/matrix_finder"
fi
manifest="$state_dir/install-manifest.txt"

# Version recorded by the last install ("" if none).
installed_version() {
    [ -f "$manifest" ] && sed -n 's/^# version //p' "$manifest" | head -1 || true
}

# Remove one installed path (file, link or the app bundle), with sudo if needed.
remove_path() {
    local p=$1 s
    # Only absolute paths below a folder, never a folder like / or the home folder itself.
    case "$p" in /*/*) ;; *) warn "not removing '$p'"; return 0 ;; esac
    [ "$p" != "$HOME" ] || return 0
    [ -e "$p" ] || [ -L "$p" ] || return 0
    s=$(sudo_for "$(dirname "$p")")
    $s rm -rf "$p"
    # Tidy up folders this left empty (never above share/ or bin/).
    local d
    d=$(dirname "$p")
    while [ "$d" != / ] && [ "$(basename "$d")" != share ] && [ "$(basename "$d")" != bin ] && rmdir "$d" 2>/dev/null; do
        d=$(dirname "$d")
    done
    return 0
}

# Linux: let the desktop see a menu entry or icon that came or went.
refresh_menus() {
    local share=$1 s
    s=$(sudo_for "$share")
    if [ -d "$share/applications" ] && command -v update-desktop-database >/dev/null; then
        $s update-desktop-database -q "$share/applications" 2>/dev/null || true
    fi
    if [ -d "$share/icons/hicolor" ] && command -v gtk-update-icon-cache >/dev/null; then
        $s gtk-update-icon-cache -q -f -t "$share/icons/hicolor" 2>/dev/null || true
    fi
    return 0
}

# ---------------------------------------------------------------- uninstall

if [ "$mode" = uninstall ]; then
    [ -f "$manifest" ] || die "nothing to uninstall: $manifest not found (an app installed from the DMG can be dragged to the Trash)"
    say "Removing Matrix Finder $(installed_version)"
    grep -v '^#' "$manifest" | while IFS= read -r p; do
        [ -n "$p" ] || continue
        echo "    $p"
        remove_path "$p"
    done
    $(sudo_for "$state_dir") rm -f "$manifest"
    rmdir "$state_dir" 2>/dev/null || true
    if [ "$os" = linux ]; then refresh_menus "${prefix:-$HOME/.local}/share"; fi
    say "Done. Settings were kept; delete them by hand if you like:"
    if [ "$os" = mac ]; then echo "    $HOME/Library/Application Support/matrix_finder"; else echo "    ${XDG_CONFIG_HOME:-$HOME/.config}/matrix_finder"; fi
    exit 0
fi

# ---------------------------------------------------------------- system packages

install_linux_packages() {
    local id="" like="" pm="" missing="" p
    if [ -r /etc/os-release ]; then
        id=$(. /etc/os-release && echo "${ID:-}")
        like=$(. /etc/os-release && echo "${ID_LIKE:-}")
    fi
    # Build: C/C++ compiler (unrar, sqlite, libssh2), pkg-config and OpenSSL headers (SFTP), git.
    # Run: OpenGL/EGL, xkbcommon, Wayland and X11 client libraries (loaded at start).
    case " $id $like " in
        *" debian "*|*" ubuntu "*)
            pm=apt
            for p in build-essential pkg-config libssl-dev git ca-certificates curl \
                     libgl1 libegl1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
                     libx11-6 libxcursor1 libxrandr2 libxi6; do
                grep -q '^ii' <<<"$(dpkg-query -W -f='${db:Status-Abbrev}' "$p" 2>/dev/null)" || missing="$missing $p"
            done ;;
        *" fedora "*|*" rhel "*|*" centos "*)
            pm=dnf
            for p in gcc gcc-c++ make pkgconf-pkg-config openssl-devel git ca-certificates curl \
                     mesa-libGL mesa-libEGL libxkbcommon libxkbcommon-x11 libwayland-client libwayland-cursor libwayland-egl \
                     libX11 libXcursor libXrandr libXi; do
                rpm -q "$p" >/dev/null 2>&1 || missing="$missing $p"
            done ;;
        *)
            warn "unknown distribution '$id': install a C/C++ compiler, make, pkg-config, the OpenSSL headers, git and curl yourself"
            return 0 ;;
    esac
    [ -n "$missing" ] || return 0
    local s
    s=$(sudo_for /usr)
    if [ "$deps" = 0 ] || ! ask "Install the packages Matrix Finder needs:$missing ?"; then
        if [ "$pm" = apt ]; then warn "missing packages; install them with: sudo apt-get install$missing"
        else warn "missing packages; install them with: sudo dnf install$missing"; fi
        warn "continuing anyway; the build may fail"
        return 0
    fi
    say "Installing:$missing"
    # shellcheck disable=SC2086
    if [ "$pm" = apt ]; then
        $s env DEBIAN_FRONTEND=noninteractive apt-get update -q
        $s env DEBIAN_FRONTEND=noninteractive apt-get install -y -q $missing
    else
        $s dnf install -y $missing
    fi
}

check_mac_tools() {
    if ! xcode-select -p >/dev/null 2>&1; then
        warn "the Xcode command line tools (compiler, git) are missing"
        if [ "$deps" = 1 ] && ask "Open the installer for them now?"; then
            xcode-select --install || true
        fi
        die "install the Xcode command line tools (xcode-select --install), then run this script again"
    fi
    check_mac_linker
}

# Links a tiny program with `cc`, which is what rustc links with. Catches a linker
# that is older than the SDK beside it (a half-finished Command Line Tools update:
# "tapi error: malformed file ... unknown architecture") before a long build. When
# an older SDK in the same folder works, the build uses that one instead.
link_test() {
    local dir out
    dir=$(mktemp -d)
    printf 'int main(void) { return 0; }\n' >"$dir/t.c"
    if out=$(cc -arch "$(uname -m)" "$dir/t.c" -o "$dir/t" 2>&1); then rm -rf "$dir"; return 0; fi
    rm -rf "$dir"
    printf '%s\n' "$out" | grep -v '^ *\^' | head -4 | sed 's/^/    /' >&2
    return 1
}

check_mac_linker() {
    command -v cc >/dev/null || die "no C compiler (cc) found; install the Xcode command line tools (xcode-select --install)"
    link_test 2>/dev/null && return 0
    local dev cc_path ldv current sdks s d
    dev=$(xcode-select -p)
    cc_path=$(command -v cc)
    ldv=$(ld -v 2>&1 | sed -n 's/.*PROJECT:\([^ ]*\).*/\1/p' | head -1 || true)
    current=$(cd -P "${SDKROOT:-$(xcrun --show-sdk-path 2>/dev/null || echo /nonexistent)}" 2>/dev/null && pwd || true)
    warn "the C compiler cannot link a test program with the macOS SDK${current:+ ${current##*/}}:"
    link_test || true
    # Another SDK of the same tools, newest first (real folders, not the MacOSX.sdk links).
    for d in "$dev/SDKs" "$dev/Platforms/MacOSX.platform/Developer/SDKs"; do
        [ -d "$d" ] || continue
        sdks=$(cd "$d" && for s in MacOSX[0-9]*.sdk; do if [ -d "$s" ] && [ ! -L "$s" ]; then echo "$s"; fi; done |
            sed 's/^MacOSX//; s/\.sdk$//' | sort -t. -k1,1nr -k2,2nr)
        for s in $sdks; do
            [ "$d/MacOSX$s.sdk" = "$current" ] && continue
            if SDKROOT="$d/MacOSX$s.sdk" link_test 2>/dev/null; then
                export SDKROOT="$d/MacOSX$s.sdk"
                warn "building with the older SDK MacOSX$s.sdk, which works"
                warn "to fix the tools for good, update the Command Line Tools (System Settings > General > Software Update)"
                return 0
            fi
        done
    done
    printf '\n' >&2
    if [ "$cc_path" != /usr/bin/cc ]; then
        warn "cc is $cc_path, not Apple's /usr/bin/cc: take it off PATH (e.g. conda deactivate) and try again"
    fi
    case $dev in
        /Library/Developer/CommandLineTools*)
            die "the Command Line Tools at $dev look half-updated (linker ${ldv:-unknown}, SDK ${current##*/}).
Update them in System Settings > General > Software Update, or reinstall them:
    sudo rm -rf /Library/Developer/CommandLineTools && xcode-select --install
then run this script again" ;;
        *)
            die "the linker of $dev (${ldv:-unknown}) cannot use the SDK ${current##*/}.
Update Xcode, or use the Command Line Tools instead:
    sudo xcode-select -s /Library/Developer/CommandLineTools
then run this script again" ;;
    esac
}

# ---------------------------------------------------------------- source

# This checkout, when the script is run from one (and no release was asked for).
script=${BASH_SOURCE[0]:-}
src=""
if [ -z "$ref" ] && [ "$want_latest" = 0 ] && [ -n "$script" ] && [ -f "$script" ]; then
    root=$(cd "$(dirname "$script")/.." && pwd)
    if [ -f "$root/Cargo.toml" ] && [ "$(cargo_field "$root/Cargo.toml" name)" = matrix_finder ]; then
        src=$root
    fi
fi

if [ "$os" = mac ]; then check_mac_tools; else install_linux_packages; fi

if [ -z "$src" ]; then
    command -v git >/dev/null || die "git is needed to download the source"
    if [ -z "$ref" ]; then
        # The newest release tag vX.Y.Z (pre-releases such as v1.0.0-rc1 are skipped).
        tags=$(git ls-remote --tags --refs "$REPO" 'v*' | sed -n 's|.*refs/tags/\(v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$|\1|p')
        [ -n "$tags" ] || die "no release tags found at $REPO"
        ref=""
        for t in $tags; do
            if [ -z "$ref" ] || [ "$(ver_cmp "$t" "$ref")" = 1 ]; then ref=$t; fi
        done
    fi
    # One clone kept between runs: an upgrade only fetches what changed, and the build
    # reuses what it compiled last time.
    src="$cache_dir/src"
    if [ -d "$src/.git" ] && [ "$(git -C "$src" remote get-url origin 2>/dev/null)" = "$REPO" ]; then
        say "Fetching $REPO"
        git -C "$src" fetch --quiet --force --tags --prune origin '+refs/heads/*:refs/remotes/origin/*'
    else
        say "Downloading $REPO"
        rm -rf "$src"
        mkdir -p "$cache_dir"
        git clone --quiet --filter=blob:none --no-checkout "$REPO" "$src"
    fi
    # A branch name means its newest commit on GitHub.
    target=$ref
    if git -C "$src" rev-parse --verify --quiet "refs/remotes/origin/$ref" >/dev/null; then target="origin/$ref"; fi
    git -C "$src" rev-parse --verify --quiet "$target^{commit}" >/dev/null || die "$ref not found in $REPO"
    git -C "$src" -c advice.detachedHead=false checkout --quiet --force --detach "$target"
    git -C "$src" clean -ffdxq -e /target
fi

cd "$src"
[ "$(cargo_field Cargo.toml name)" = matrix_finder ] || die "$src is not the Matrix Finder source"
version=$(cargo_field Cargo.toml version)
bin=matrix_finder
say "Matrix Finder $version (source: $src${ref:+, $ref})"

# ---------------------------------------------------------------- Rust

need_rust=$(cargo_field Cargo.toml rust-version)
need_rust=${need_rust:-1.0}
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

rust_ok() {
    command -v cargo >/dev/null && command -v rustc >/dev/null || return 1
    local v
    v=$(rustc --version | awk '{print $2}')
    [ "$(ver_cmp "$v" "$need_rust")" != -1 ]
}

if ! rust_ok; then
    if command -v rustup >/dev/null; then
        # The default toolchain is too old (or pinned): build with current stable.
        say "Rust $need_rust or newer is needed; installing the stable toolchain with rustup"
        rustup toolchain install stable --profile minimal
        export RUSTUP_TOOLCHAIN=stable
    elif [ "$deps" = 1 ] && command -v curl >/dev/null && ask "Rust $need_rust or newer is needed. Install it with rustup (https://rustup.rs) into ~/.cargo?"; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
        . "$HOME/.cargo/env"
        export RUSTUP_TOOLCHAIN=stable
    fi
    rust_ok || die "Rust $need_rust or newer is needed: install it from https://rustup.rs and run this script again"
fi
say "Using $(rustc --version)"

# --locked keeps the dependency versions the release was tested with; a source whose
# Cargo.lock is out of date builds without it rather than failing.
locked=--locked
if grep -q 'lock file.*needs to be updated' <<<"$(cargo metadata --locked --format-version 1 2>&1 >/dev/null)"; then
    warn "Cargo.lock does not match Cargo.toml; building with updated dependencies"
    locked=""
fi

# ---------------------------------------------------------------- build and install

new_files=""   # what this install wrote, one path per line
add_file() { new_files="$new_files$1
"; }

# Copy FILE to DEST through a temporary name, so that a running copy is not touched
# ("text file busy") and DEST is never half written.
put() {
    local from=$1 dest=$2 mode=$3 s tmp
    s=$(sudo_for "$(dirname "$dest")")
    $s mkdir -p "$(dirname "$dest")"
    tmp="$dest.new.$$"
    $s cp "$from" "$tmp"
    $s chmod "$mode" "$tmp"
    $s mv -f "$tmp" "$dest"
    add_file "$dest"
}

running=0
if pgrep -x "$bin" >/dev/null 2>&1; then running=1; fi
previous=$(installed_version)
if [ -n "$previous" ] && [ "$(ver_cmp "$version" "$previous")" = -1 ]; then
    warn "installing $version over the newer $previous"
fi

if [ "$os" = mac ]; then
    [ -x scripts/macos/build-dmg.sh ] || die "scripts/macos/build-dmg.sh is missing in this version"
    say "Building the app (a few minutes the first time)"
    # The bundle script reads target/ in the source folder, whatever CARGO_TARGET_DIR says.
    env -u CARGO_TARGET_DIR FEATURES="${FEATURES:---features vendored-openssl} $locked" \
        scripts/macos/build-dmg.sh --app-only
    built=""
    for a in target/macos/*.app; do [ -d "$a" ] && built=$a; done
    [ -n "$built" ] || die "the build made no app in target/macos"
    app_base=$(basename "$built")

    if [ -z "$app_dir" ]; then
        if [ -w /Applications ]; then app_dir=/Applications; else app_dir="$HOME/Applications"; fi
    fi
    mkdir -p "$app_dir" 2>/dev/null || $(sudo_for "$app_dir") mkdir -p "$app_dir"
    dest="$app_dir/$app_base"
    if [ -d "$dest" ]; then
        id=$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$dest/Contents/Info.plist" 2>/dev/null || true)
        [ -z "$id" ] || [ "$id" = "$APP_ID" ] || die "$dest is another app ($id); move it away or use --app-dir"
    fi
    say "Installing $dest"
    s=$(sudo_for "$app_dir")
    # Put the new app next to the old one, then swap: the old one keeps running if open.
    $s rm -rf "$dest.new" "$dest.old"
    $s ditto "$built" "$dest.new"
    [ -d "$dest" ] && $s mv "$dest" "$dest.old"
    $s mv "$dest.new" "$dest"
    $s rm -rf "$dest.old"
    add_file "$dest"
    # Refresh the Finder/Dock icon and version of the app.
    lsreg=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    [ -x "$lsreg" ] && "$lsreg" -f "$dest" >/dev/null 2>&1 || true

    # The matrix_finder command: a link into the app, so it follows every upgrade.
    if [ -n "$prefix" ]; then bin_dir="$prefix/bin"
    elif [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then bin_dir=/usr/local/bin
    else bin_dir="$HOME/.local/bin"; fi
    link="$bin_dir/$bin"
    if [ -e "$link" ] && [ ! -L "$link" ]; then
        warn "$link exists and is not a link; not replacing it"
    else
        s=$(sudo_for "$bin_dir")
        $s mkdir -p "$bin_dir"
        $s ln -sfn "$dest/Contents/MacOS/$bin" "$link"
        add_file "$link"
    fi
else
    say "Building (a few minutes the first time)"
    # shellcheck disable=SC2086
    env -u CARGO_TARGET_DIR cargo build --release $locked ${FEATURES:-}
    built="target/release/$bin"
    [ -x "$built" ] || die "the build made no $built"

    prefix=${prefix:-$HOME/.local}
    bin_dir="$prefix/bin"
    say "Installing into $prefix"
    put "$built" "$bin_dir/$bin" 755

    # Icons: every assets/logo/<name>-<size>.png (the "small" ones are drawn for small
    # sizes), plus the SVG for any other size.
    share="$prefix/share"
    for f in assets/logo/*.png; do
        [ -f "$f" ] || continue
        b=$(basename "$f" .png)
        case "$b" in *macos*) continue ;; esac
        n=${b##*-}
        case "$n" in ''|*[!0-9]*) continue ;; esac
        put "$f" "$share/icons/hicolor/${n}x${n}/apps/$APP_ID.png" 644
    done
    [ -f assets/logo/matrix_finder.svg ] && put assets/logo/matrix_finder.svg "$share/icons/hicolor/scalable/apps/$APP_ID.svg" 644

    # The dock finds this entry by its file name on Wayland (the window's app id is
    # $APP_ID) and by StartupWMClass on X11, where the window class is the program's
    # name: egui sets the app id only on Wayland.
    desktop=$(mktemp)
    cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=$APP_NAME
GenericName=File Manager
Comment=Dual-panel file manager with tabs, a terminal and previews
Exec="$bin_dir/$bin" %f
Icon=$APP_ID
Terminal=false
StartupNotify=true
StartupWMClass=$bin
Categories=System;FileTools;FileManager;
Keywords=files;folders;file manager;dual panel;commander;terminal;
Actions=new-window;

[Desktop Action new-window]
Name=New Window
Exec="$bin_dir/$bin" --new-window
EOF
    put "$desktop" "$share/applications/$APP_ID.desktop" 644
    rm -f "$desktop"

    refresh_menus "$share"
fi

# ---------------------------------------------------------------- manifest

# Files the earlier install had and this one does not (renamed icons, a moved app…).
if [ -f "$manifest" ]; then
    grep -v '^#' "$manifest" | while IFS= read -r p; do
        [ -n "$p" ] || continue
        if ! grep -qxF "$p" <<<"$new_files"; then
            echo "    removing old $p"
            remove_path "$p"
        fi
    done
fi
m=$(mktemp)
{ echo "# Matrix Finder install manifest: files written by scripts/install.sh"; echo "# version $version"; printf '%s' "$new_files"; } >"$m"
s=$(sudo_for "$state_dir")
$s mkdir -p "$state_dir"
$s cp "$m" "$manifest"
$s chmod 644 "$manifest"
rm -f "$m"

# ---------------------------------------------------------------- done

installed="$bin_dir/$bin"
v=$("$installed" --version 2>/dev/null || true)
[ "$v" = "matrix_finder $version" ] || warn "the installed program reports '$v', expected 'matrix_finder $version'"

case "${previous:+$(ver_cmp "$version" "$previous")}" in
    1) say "Upgraded Matrix Finder $previous -> $version" ;;
    -1) say "Downgraded Matrix Finder $previous -> $version" ;;
    *) say "Installed Matrix Finder $version" ;;
esac
if [ "$os" = mac ]; then echo "    app:     $dest"; fi
echo "    command: $installed"
on_path "$bin_dir" || echo "    note: $bin_dir is not on your PATH; add it to use the matrix_finder command"
other=$(command -v "$bin" 2>/dev/null || true)
if [ -n "$other" ] && [ "$other" != "$installed" ] && on_path "$bin_dir"; then
    warn "'$bin' runs $other, which comes first on your PATH (another install, or one from cargo install?)"
fi
if [ "$running" = 1 ]; then
    warn "Matrix Finder is running: quit it and open it again to use $version (until then a new launch opens in the running window)"
fi
