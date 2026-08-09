#!/usr/bin/env bash
# install.sh — bootstrap Rust + system deps, build, and install ishell (termusic + termusic-server)
#
# There are no prebuilt binaries for this fork, so this script compiles from source. What it
# automates so you don't have to do anything by hand:
#   1. Detect the platform: Linux, Termux (Android), or Windows (Git Bash / MSYS2).
#   2. Install the Rust toolchain (rustup) if `cargo` isn't already on PATH.
#   3. Install the system build dependencies needed to compile (best-effort, via the detected
#      package manager).
#   4. `cargo install` both binaries (termusic, termusic-server) into `~/.cargo/bin`, and make
#      sure that directory is on PATH.
#
# Usage:
#   ./install.sh          interactive (asks before running `sudo`)
#   ./install.sh -y        non-interactive (assumes yes to every prompt)
#
# Known caveats (see the printed summary at the end too):
#   - Windows needs a C++ linker (MSVC Build Tools) already installed; this script only checks
#     for one and prints instructions — installing Visual Studio Build Tools automatically is too
#     heavy/risky to do without you explicitly asking for it.
#   - Termux: audio output is NOT guaranteed. cpal's Android backend expects a JNI/Activity
#     context that a plain Termux process doesn't provide — the app will compile and run, but
#     playback may end up silent. This is a known open question, not something this script fixes.

set -euo pipefail

ASSUME_YES=0
for arg in "$@"; do
    case "$arg" in
        -y|--yes) ASSUME_YES=1 ;;
        -h|--help)
            sed -n '2,25p' "$0"
            exit 0
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ---------------------------------------------------------------------------
# small helpers
# ---------------------------------------------------------------------------

info()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()   { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

confirm() {
    local prompt="$1"
    if [ "$ASSUME_YES" -eq 1 ]; then
        return 0
    fi
    read -r -p "$prompt [y/N] " reply
    case "$reply" in
        y|Y|yes|YES) return 0 ;;
        *) return 1 ;;
    esac
}

run_sudo() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
        return
    fi
    if ! command -v sudo >/dev/null 2>&1; then
        die "need root to run: $* (no 'sudo' found — re-run this script as root)"
    fi
    if confirm "Run as root: $*"; then
        sudo "$@"
    else
        die "declined to run: $* — install those packages yourself and re-run this script."
    fi
}

# ---------------------------------------------------------------------------
# 1. platform detection
# ---------------------------------------------------------------------------

detect_platform() {
    if [ -n "${TERMUX_VERSION:-}" ] || [ -d "/data/data/com.termux" ]; then
        echo "termux"
    elif [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == cygwin* || "${OSTYPE:-}" == win32* ]]; then
        echo "windows"
    elif [[ "$(uname -s)" == Linux ]]; then
        echo "linux"
    else
        echo "unsupported"
    fi
}

PLATFORM="$(detect_platform)"
info "Detected platform: $PLATFORM"

if [ "$PLATFORM" = "unsupported" ]; then
    die "This script only supports Linux, Termux, and Windows (via Git Bash / MSYS2). Detected: $(uname -a)"
fi

# ---------------------------------------------------------------------------
# 2. system build dependencies (best-effort, skipped if already satisfied)
# ---------------------------------------------------------------------------

install_system_deps() {
    case "$PLATFORM" in
        termux)
            info "Installing Termux packages (rust, clang, pkg-config, binutils, make, openssl, protobuf)..."
            pkg update
            # openssl: the project builds against native-tls (not rustls) on Android specifically,
            # since rustls's default crypto backend (aws-lc-rs) doesn't build reliably against
            # Termux's clang — see the comment on the `reqwest`/`stream-download` deps in
            # playback/Cargo.toml.
            # protobuf: provides `protoc`, needed at build time to compile the gRPC .proto files.
            pkg install -y rust clang pkg-config binutils make openssl protobuf
            ;;
        linux)
            if command -v apt-get >/dev/null 2>&1; then
                info "Installing apt packages (build-essential, pkg-config, libasound2-dev, protobuf-compiler, curl)..."
                run_sudo apt-get update -y
                run_sudo apt-get install -y build-essential pkg-config libasound2-dev protobuf-compiler curl
            elif command -v dnf >/dev/null 2>&1; then
                info "Installing dnf packages..."
                run_sudo dnf install -y gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel protobuf-compiler curl
            elif command -v pacman >/dev/null 2>&1; then
                info "Installing pacman packages..."
                run_sudo pacman -Sy --noconfirm base-devel pkgconf alsa-lib protobuf curl
            elif command -v zypper >/dev/null 2>&1; then
                info "Installing zypper packages..."
                run_sudo zypper install -y gcc gcc-c++ make pkg-config alsa-devel protobuf-devel curl
            elif command -v apk >/dev/null 2>&1; then
                info "Installing apk packages..."
                run_sudo apk add build-base pkgconf alsa-lib-dev protobuf curl
            else
                warn "Unrecognized Linux package manager — skipping automatic system deps."
                warn "You'll need: a C compiler, pkg-config, ALSA development headers (e.g. libasound2-dev), and protoc (protobuf compiler)."
            fi
            ;;
        windows)
            # Nothing to apt-get on Windows; just check for a usable linker and protoc further down.
            ;;
    esac
}

install_system_deps

if ! command -v protoc >/dev/null 2>&1; then
    case "$PLATFORM" in
        windows)
            warn "protoc (protobuf compiler) not found — needed at build time to compile the .proto files."
            if command -v winget >/dev/null 2>&1; then
                warn "Install it with: winget install --id Google.Protobuf"
            else
                warn "Download it from: https://github.com/protocolbuffers/protobuf/releases"
            fi
            warn "This script won't install it automatically on Windows."
            if ! confirm "Continue anyway?"; then
                die "aborted — install protoc, then re-run this script."
            fi
            ;;
        *)
            die "protoc still not found after installing system packages — install it manually (protobuf compiler) and re-run this script."
            ;;
    esac
fi

# ---------------------------------------------------------------------------
# 3. Rust toolchain
# ---------------------------------------------------------------------------

ensure_cargo_on_path() {
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
}

if ! command -v cargo >/dev/null 2>&1; then
    case "$PLATFORM" in
        termux)
            # `rust` was just installed via pkg above; nothing more to do.
            ;;
        linux)
            info "Installing Rust via rustup..."
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
            ensure_cargo_on_path
            ;;
        windows)
            info "Installing Rust via rustup-init.exe..."
            curl -sSf -o "$SCRIPT_DIR/rustup-init.exe" https://win.rustup.rs/x86_64
            "$SCRIPT_DIR/rustup-init.exe" -y --default-toolchain stable
            rm -f "$SCRIPT_DIR/rustup-init.exe"
            ensure_cargo_on_path
            ;;
    esac
fi

ensure_cargo_on_path
command -v cargo >/dev/null 2>&1 || die "cargo still not found on PATH after installation — open a new terminal and re-run this script."

info "Using $(cargo --version) / $(rustc --version 2>/dev/null || echo 'rustc unknown')"

# ---------------------------------------------------------------------------
# 4. Windows: sanity-check for a C++ linker (MSVC Build Tools)
# ---------------------------------------------------------------------------

if [ "$PLATFORM" = "windows" ]; then
    if ! command -v link.exe >/dev/null 2>&1 && ! command -v cl.exe >/dev/null 2>&1; then
        warn "No MSVC linker (link.exe/cl.exe) found on PATH."
        warn "The build below will likely fail without it. Install 'Build Tools for Visual"
        warn "Studio' (C++ workload) first, e.g.:"
        warn "  winget install --id Microsoft.VisualStudio.2022.BuildTools --exact"
        warn "  (then, in the installer, select 'Desktop development with C++')"
        warn "This script won't do that automatically — it's a large, interactive install."
        if ! confirm "Continue anyway?"; then
            die "aborted — install the C++ Build Tools, then re-run this script."
        fi
    fi
fi

# ---------------------------------------------------------------------------
# 5. build + install
# ---------------------------------------------------------------------------

info "Building and installing termusic (this compiles the whole project — a few minutes)..."
cargo install --path tui --locked --force

info "Building and installing termusic-server..."
cargo install --path server --locked --force

# ---------------------------------------------------------------------------
# 6. make sure ~/.cargo/bin is on PATH for future shells too
# ---------------------------------------------------------------------------

CARGO_BIN="$HOME/.cargo/bin"
add_path_line_to() {
    local rcfile="$1"
    local line='export PATH="$HOME/.cargo/bin:$PATH"'
    touch "$rcfile"
    grep -qF "$line" "$rcfile" 2>/dev/null || { echo "$line" >> "$rcfile"; info "Added ~/.cargo/bin to PATH in $rcfile"; }
}

for rcfile in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
    add_path_line_to "$rcfile"
done

case ":$PATH:" in
    *":$CARGO_BIN:"*) ;;
    *) export PATH="$CARGO_BIN:$PATH" ;;
esac

# ---------------------------------------------------------------------------
# 7. done
# ---------------------------------------------------------------------------

echo
if command -v termusic >/dev/null 2>&1; then
    info "Installed! Run: termusic"
else
    warn "Build finished, but 'termusic' isn't on PATH in this shell yet."
    warn "Open a new terminal (or 'source ~/.bashrc') and run: termusic"
fi

if [ "$PLATFORM" = "termux" ]; then
    echo
    warn "Termux note: audio output isn't guaranteed here. cpal's Android backend generally"
    warn "expects a JNI/Activity context a plain Termux process doesn't have, so the app may"
    warn "run silently. If that happens, it's a known limitation, not a broken install."
fi
