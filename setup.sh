#!/usr/bin/env bash
# Workstation setup for TentaBrowser.
#
# Installs what the commands in AGENTS.md need: the system build dependencies,
# the Rust toolchain pinned in rust-toolchain.toml, the cargo tools the gates
# use, and the native V8 bundle, which it builds from source when the cache has
# none (crates/tenta-v8/native/BUILDING.md). Safe to re-run: every step checks
# before it changes anything.
#
# Supported: macOS (Homebrew), Fedora, Debian/Ubuntu and Arch/CachyOS, with
# their derivatives. Elsewhere it lists what to install by hand and continues
# with the Rust part.
#
# Written for the bash 3.2 that macOS ships: no associative arrays, no mapfile.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

CHECK=0
ALL=0
SYSTEM=1
V8=1
REBUILD_V8=0
MISSING=0
FAILED=0

usage() {
    cat <<'EOF'
Usage: ./setup.sh [options]

  --check       report what is missing and the commands that would run; change nothing
  --all         also install Node.js, cargo-about and CJK fonts (golden tracer,
                WPT Chromium driver, CDP client smoke test, `cargo xtask notices`,
                screenshots of Chinese, Japanese and Korean pages)
  --no-system   skip system packages (no sudo); only Rust, the cargo tools and V8
  --no-v8       skip the V8 bundle; builds without the `v8` feature still work
  --rebuild-v8  build V8 here even though a bundle for this target is already
                installed, and move this machine's local pin to the new build
  -h, --help    this text

Exit status: 0 ready, 1 something is missing or failed, 2 usage.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --check) CHECK=1 ;;
        --all) ALL=1 ;;
        --no-system) SYSTEM=0 ;;
        --no-v8) V8=0 ;;
        --rebuild-v8) REBUILD_V8=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
    shift
done

if [ -t 1 ]; then
    BOLD=$'\033[1m' GREEN=$'\033[32m' YELLOW=$'\033[33m' RED=$'\033[31m' RESET=$'\033[0m'
else
    BOLD="" GREEN="" YELLOW="" RED="" RESET=""
fi

section() { printf '\n%s== %s%s\n' "$BOLD" "$1" "$RESET"; }
ok() { printf '  %sok%s       %s\n' "$GREEN" "$RESET" "$1"; }
note() { printf '  %snote%s     %s\n' "$YELLOW" "$RESET" "$1"; }
missing() {
    printf '  %smissing%s  %s\n' "$RED" "$RESET" "$1"
    MISSING=$((MISSING + 1))
}
failed() {
    printf '  %sfailed%s   %s\n' "$RED" "$RESET" "$1"
    FAILED=$((FAILED + 1))
}
die() {
    printf '%ssetup.sh: %s%s\n' "$RED" "$1" "$RESET" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

# Print a command; run it unless this is a --check run.
run() {
    printf '  $ %s\n' "$*"
    if [ "$CHECK" = 0 ]; then
        "$@"
    fi
}

# ---------------------------------------------------------------- platform --

PLATFORM=unknown
detect_platform() {
    case "$(uname -s)" in
        Darwin)
            PLATFORM=macos
            return
            ;;
        Linux) ;;
        *) return ;;
    esac
    local ids="" id
    if [ -r /etc/os-release ]; then
        # shellcheck source=/dev/null
        ids="$(. /etc/os-release && printf '%s %s' "${ID:-}" "${ID_LIKE:-}")"
    fi
    for id in $ids; do
        case "$id" in
            fedora | rhel | centos) PLATFORM=fedora ;;
            debian | ubuntu) PLATFORM=debian ;;
            arch | cachyos | manjaro | endeavouros) PLATFORM=arch ;;
            *) continue ;;
        esac
        return
    done
    # An unlisted distribution: go by its package manager.
    if have dnf; then
        PLATFORM=fedora
    elif have apt-get; then
        PLATFORM=debian
    elif have pacman; then
        PLATFORM=arch
    fi
}

python_at_least() { # MINOR
    have python3 && python3 -c "import sys; sys.exit(sys.version_info < (3, $1))"
}

# ---------------------------------------------------------- system packages --

# What each package is for:
#   C/C++ compiler, make, cmake  BoringSSL (boring-sys), QuickJS, zstd
#   clang + libclang             bindgen (boring-sys)
#   pkg-config, fontconfig       system font fallback of the render feature
#   python3 (3.11+)              Stylo's build script, bench/, wpt/, idl/ tooling
#   patch, git, curl, openssl    source recipes, WPT checkout and its server
#   fonts                        screenshots of pages that name no web font
#   Node.js (--all)              golden tracer, WPT Chromium driver, CDP smoke
#   CJK fonts (--all)            large; without them CJK text renders as boxes
system_packages() {
    local pkgs=""
    case "$PLATFORM" in
        fedora)
            pkgs="gcc gcc-c++ make cmake clang clang-devel pkgconf-pkg-config fontconfig-devel"
            pkgs="$pkgs python3 git curl openssl patch"
            pkgs="$pkgs dejavu-sans-fonts liberation-sans-fonts liberation-serif-fonts"
            pkgs="$pkgs liberation-mono-fonts google-noto-sans-fonts google-noto-color-emoji-fonts"
            if [ "$ALL" = 1 ]; then pkgs="$pkgs nodejs nodejs-npm google-noto-sans-cjk-fonts"; fi
            ;;
        debian)
            pkgs="build-essential cmake clang libclang-dev pkg-config libfontconfig-dev"
            pkgs="$pkgs python3 git curl ca-certificates openssl patch"
            pkgs="$pkgs fonts-dejavu-core fonts-liberation fonts-noto-core fonts-noto-color-emoji"
            if [ "$ALL" = 1 ]; then pkgs="$pkgs nodejs npm fonts-noto-cjk"; fi
            ;;
        arch)
            pkgs="base-devel cmake clang fontconfig python git curl openssl"
            pkgs="$pkgs ttf-dejavu ttf-liberation noto-fonts noto-fonts-emoji"
            if [ "$ALL" = 1 ]; then pkgs="$pkgs nodejs npm noto-fonts-cjk"; fi
            ;;
        macos)
            # The compiler and libclang come with the Xcode Command Line Tools.
            pkgs="cmake"
            if ! python_at_least 11; then pkgs="$pkgs python"; fi
            if [ "$ALL" = 1 ]; then pkgs="$pkgs node"; fi
            ;;
    esac
    printf '%s' "$pkgs"
}

package_installed() {
    case "$PLATFORM" in
        fedora) rpm -q --whatprovides "$1" >/dev/null 2>&1 ;;
        debian) dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q 'ok installed' ;;
        arch) pacman -T "$1" >/dev/null 2>&1 ;;
        macos) brew list --versions "$1" >/dev/null 2>&1 ;;
    esac
}

SUDO=""
require_root() {
    if [ "$(id -u)" = 0 ]; then
        SUDO=""
    elif have sudo; then
        SUDO="sudo"
    elif [ "$CHECK" = 0 ]; then
        die "installing system packages needs root: run as root, install sudo, or pass --no-system"
    fi
}

install_packages() { # PACKAGE...
    case "$PLATFORM" in
        fedora) run $SUDO dnf install -y "$@" ;;
        debian)
            run $SUDO apt-get update
            run $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y "$@"
            ;;
        arch)
            # A fresh container or chroot has no package database yet. An
            # installed system keeps its own: refreshing it here without a
            # full upgrade would leave a partial upgrade behind.
            if ! ls /var/lib/pacman/sync/*.db >/dev/null 2>&1; then
                run $SUDO pacman -Sy
            fi
            run $SUDO pacman -S --needed --noconfirm "$@" ||
                die "pacman failed; if it could not download a package, run 'sudo pacman -Syu' and try again"
            ;;
        macos) run brew install "$@" ;;
    esac
}

setup_system() {
    section "System packages ($PLATFORM)"
    if [ "$SYSTEM" = 0 ]; then
        note "skipped (--no-system)"
        return
    fi
    if [ "$PLATFORM" = unknown ]; then
        note "unrecognised system; install these by hand:"
        note "a C and C++ compiler, make, cmake, clang with libclang, pkg-config,"
        note "fontconfig headers, python 3.11+, git, curl, openssl, patch"
        return
    fi
    if [ "$PLATFORM" = macos ]; then
        if xcode-select -p >/dev/null 2>&1; then
            ok "Xcode Command Line Tools"
        else
            missing "Xcode Command Line Tools"
            [ "$CHECK" = 1 ] || die "run 'xcode-select --install', then run this script again"
        fi
        if ! have brew; then
            missing "Homebrew"
            [ "$CHECK" = 1 ] || die "install Homebrew (https://brew.sh), then run this script again"
            return
        fi
    fi

    local pkg wanted=""
    for pkg in $(system_packages); do
        if package_installed "$pkg"; then
            ok "$pkg"
        else
            missing "$pkg"
            wanted="$wanted $pkg"
        fi
    done
    if [ -n "$wanted" ]; then
        [ "$PLATFORM" = macos ] || require_root
        # shellcheck disable=SC2086 # the list is split on purpose
        install_packages $wanted
    fi

    if [ "$CHECK" = 0 ] && ! python_at_least 11; then
        failed "python3 is older than 3.11; the bench, WPT and IDL tooling needs tomllib"
    fi
    if [ "$ALL" = 1 ] && have node; then
        local major
        major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
        if [ "$major" -lt 20 ]; then
            note "node $major is older than the 20 that docs/dev/wpt.md asks for"
        fi
    fi
}

# --------------------------------------------------------------------- rust --

TOOLCHAIN="$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$REPO_ROOT/rust-toolchain.toml")"
[ -n "$TOOLCHAIN" ] || die "no channel in $REPO_ROOT/rust-toolchain.toml"

toolchain_ready() {
    rustup toolchain list 2>/dev/null | grep -q "^$TOOLCHAIN-" || return 1
    local installed
    installed="$(rustup component list --toolchain "$TOOLCHAIN" --installed 2>/dev/null)" || return 1
    printf '%s\n' "$installed" | grep -q '^rustfmt' || return 1
    printf '%s\n' "$installed" | grep -q '^clippy' || return 1
}

setup_rust() {
    section "Rust $TOOLCHAIN"
    # A rustup from an earlier run is not on PATH until the shell restarts.
    case ":$PATH:" in
        *":$CARGO_BIN:"*) ;;
        *) PATH="$CARGO_BIN:$PATH" ;;
    esac

    if have rustup; then
        ok "rustup"
    else
        missing "rustup"
        have curl || [ "$CHECK" = 1 ] || die "curl is needed to install rustup"
        printf '  $ curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none\n'
        if [ "$CHECK" = 0 ]; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
                sh -s -- -y --default-toolchain none
        fi
    fi

    if have rustup && toolchain_ready; then
        ok "toolchain $TOOLCHAIN with rustfmt and clippy"
    else
        missing "toolchain $TOOLCHAIN with rustfmt and clippy"
        run rustup toolchain install "$TOOLCHAIN" --profile minimal \
            --component rustfmt --component clippy
    fi

    if [ "$CHECK" = 0 ]; then
        local active
        active="$(cd "$REPO_ROOT" && rustc --version 2>/dev/null || true)"
        case "$active" in
            "rustc $TOOLCHAIN "*) ;;
            *) failed "rustc in this repository is '$active', not $TOOLCHAIN: is a non-rustup Rust first on PATH?" ;;
        esac
    fi
}

setup_cargo_tools() {
    section "Cargo tools"
    local tools="cargo-nextest cargo-deny" tool
    if [ "$ALL" = 1 ]; then tools="$tools cargo-about"; fi
    for tool in $tools; do
        if have "$tool"; then
            ok "$tool"
            continue
        fi
        missing "$tool"
        # cargo-about builds its binary only with the `cli` feature.
        local features=""
        if [ "$tool" = cargo-about ]; then features="--features cli"; fi
        printf '  $ cargo +%s install --locked %s %s\n' "$TOOLCHAIN" "$tool" "$features"
        if [ "$CHECK" = 0 ]; then
            # From $HOME, so the repository's `-D warnings` rustflags do not
            # apply to somebody else's crate. Cargo exits 0 when it installed
            # no binary, so look for the tool afterwards.
            # shellcheck disable=SC2086 # the feature flag is split on purpose
            (cd "$HOME" && cargo "+$TOOLCHAIN" install --locked "$tool" $features) || true
            have "$tool" || failed "cargo install $tool"
        fi
    done
}

# ----------------------------------------------------------------------- v8 --

V8_NATIVE="$REPO_ROOT/crates/tenta-v8/native"
V8_CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/tentabrowser/v8"
V8_BUILD="$V8_CACHE/build"
V8_SOURCE="$V8_BUILD/source/v8-152.2.0-native30"
V8_NATIVE_TESTS="web_idl_legacy web_idl_order ordinary_object_brand termination_now"
V8_DIR=""

host_target() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
        Linux-aarch64) echo aarch64-unknown-linux-gnu ;;
        Darwin-arm64) echo aarch64-apple-darwin ;;
        Darwin-x86_64) echo x86_64-apple-darwin ;;
        *) echo unknown ;;
    esac
}

v8_find_bundle() { # TARGET
    local found=""
    if [ -n "${TENTA_V8_ARTIFACT_DIR:-}" ]; then
        found="$(python3 "$V8_NATIVE/install_bundle.py" find --target "$1" \
            --directory "$TENTA_V8_ARTIFACT_DIR" 2>/dev/null || true)"
    fi
    if [ -z "$found" ]; then
        found="$(python3 "$V8_NATIVE/install_bundle.py" find --target "$1" 2>/dev/null || true)"
    fi
    printf '%s' "$found"
}

v8_build() { # TARGET
    local target="$1" tests="" name
    for name in $V8_NATIVE_TESTS; do tests="$tests --test $name"; done

    if [ ! -f "$V8_SOURCE/TENTA-SOURCE-NATIVE30.json" ]; then
        run python3 "$V8_NATIVE/recipe/bootstrap_source.py" \
            --downloads "$V8_BUILD/downloads" --destination "$V8_SOURCE" || return 1
    fi
    # The build script downloads Chromium's Clang, GN, Ninja, Rust toolchain
    # and sysroot, compiles V8 and generates the binding; the native tests of
    # BUILDING.md then run against that archive.
    # shellcheck disable=SC2030,SC2031 # the build environment stays in the subshell
    (
        cd "$V8_SOURCE" || exit 1
        clang="$V8_BUILD/target/release/clang"
        if [ -z "${LIBCLANG_PATH:-}" ]; then
            if [ "$PLATFORM" = macos ]; then
                # As in the accepted macOS run: Homebrew's libclang parses for bindgen.
                LIBCLANG_PATH="$(brew --prefix llvm)/lib"
            else
                # Bindgen needs libclang 21.1+ for V8's libc++, more than most
                # distributions ship. Chromium's Rust toolchain carries the one
                # that matches its Clang, so fetch both now, as the build
                # script would, to name them before cargo starts.
                run python3 tools/clang/scripts/update.py --output-dir "$clang" || exit 1
                run python3 tools/rust_toolchain.py || exit 1
                LIBCLANG_PATH="$V8_SOURCE/third_party/rust-toolchain/lib"
                RUSTY_V8_BINDGEN_RESOURCE_DIR="$("$clang/bin/clang" -print-resource-dir)"
                export RUSTY_V8_BINDGEN_RESOURCE_DIR
            fi
        fi
        # The GN arguments are those of the accepted macOS configuration. On
        # Linux GN would also ask pkg-config for glib, which V8 as a library
        # does not use; `use_glib=false` keeps the host's headers out of it.
        # Rustflags from the environment (CI sets `-D warnings`) are not meant
        # for this crate.
        unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
        export LIBCLANG_PATH V8_FROM_SOURCE=1 SOURCE_DATE_EPOCH=1790121600
        export GN_ARGS="symbol_level=0 treat_warnings_as_errors=false"
        if [ "$PLATFORM" != macos ]; then GN_ARGS="$GN_ARGS use_glib=false"; fi
        export CARGO_TARGET_DIR="$V8_BUILD/target"
        note "V8_FROM_SOURCE=1 LIBCLANG_PATH=$LIBCLANG_PATH GN_ARGS=\"$GN_ARGS\""
        # shellcheck disable=SC2086 # the test list is split on purpose
        run cargo "+$TOOLCHAIN" nextest run --release --locked $tests --no-fail-fast
    ) || return 1

    # The pin is this machine's own record, kept beside the bundle and out of
    # the repository; artifact_pins.rs stays the table of reviewed bundles.
    local pin="--pin"
    if [ "$REBUILD_V8" = 1 ]; then pin="--pin --replace-provenance"; fi
    # shellcheck disable=SC2086
    run python3 "$V8_NATIVE/install_bundle.py" install --target "$target" \
        --source "$V8_SOURCE" --release "$V8_BUILD/target/release" \
        --native-tests "passed: $V8_NATIVE_TESTS" $pin || return 1
}

setup_v8() {
    section "V8 native bundle"
    if [ "$V8" = 0 ]; then
        note "skipped (--no-v8)"
        return
    fi
    local target
    target="$(host_target)"
    if [ "$target" = unknown ]; then
        note "no V8 route for $(uname -s) $(uname -m); builds without the v8 feature still work"
        return
    fi
    if ! python_at_least 12; then
        if [ "$CHECK" = 1 ]; then
            missing "python 3.12+ (the V8 source recipe needs it)"
        else
            failed "the V8 source recipe needs python 3.12+"
        fi
        return
    fi

    if [ "$REBUILD_V8" = 0 ]; then
        V8_DIR="$(v8_find_bundle "$target")"
        if [ -n "$V8_DIR" ]; then
            ok "bundle for $target: $V8_DIR"
            return
        fi
        missing "bundle for $target"
        if grep -q "target: \"$target\"" "$REPO_ROOT/crates/tenta-v8/artifact_pins.rs"; then
            note "artifact_pins.rs pins a reviewed bundle for $target that is not in the cache;"
            note "to use it, copy its three files into a directory under"
            note "$V8_CACHE/152.2.0/$target/default/ (BUILDING.md). Building a local one instead."
        fi
    fi

    note "building V8 from source: 10 to 60 minutes and about 5 GB under $V8_BUILD"
    if [ "$CHECK" = 1 ]; then
        note "would reconstruct the source, build it, run the native tests and pin the bundle locally"
        return
    fi
    local tool
    for tool in patch git cargo-nextest; do
        if ! have "$tool"; then
            failed "the V8 build needs $tool"
            return
        fi
    done
    # shellcheck disable=SC2031 # v8_build sets it in its own subshell, later
    if [ "$PLATFORM" = macos ] && [ -z "${LIBCLANG_PATH:-}" ] &&
        ! brew list --versions llvm >/dev/null 2>&1; then
        run brew install llvm
    fi
    mkdir -p "$V8_BUILD"
    if ! v8_build "$target"; then
        failed "V8 build (the tree is kept in $V8_BUILD; re-running continues it)"
        return
    fi
    V8_DIR="$(v8_find_bundle "$target")"
    if [ -z "$V8_DIR" ]; then
        failed "the installed bundle does not match its pin"
        return
    fi
    ok "bundle for $target: $V8_DIR"
}

# -------------------------------------------------------------------- notes --

report_repository() {
    section "Repository"
    if [ -f "$REPO_ROOT/docs/spec/00-index.md" ]; then
        ok "docs/spec (the specification)"
    else
        note "docs/spec is absent: it is kept out of the remote, copy it from a machine that has it"
    fi
    if [ -d "$REPO_ROOT/../obscura-reference" ]; then
        ok "../obscura-reference (benchmark reference)"
    else
        note "../obscura-reference is absent: only 'bench/run.py --compare reference' needs it"
    fi
}

summary() {
    section "Result"
    if [ "$CHECK" = 1 ]; then
        if [ "$MISSING" = 0 ]; then
            ok "nothing is missing"
            return 0
        fi
        printf '  %s item(s) missing; run ./setup.sh without --check to install them\n' "$MISSING"
        return 1
    fi
    if [ "$FAILED" != 0 ]; then
        printf '  %s step(s) failed, see above\n' "$FAILED"
        return 1
    fi
    ok "ready"
    cat <<EOF

  New shells pick cargo up by themselves; in this one run:
    . "${CARGO_HOME:-$HOME/.cargo}/env"

  Then:
    cargo build --release -p tentabrowser-cli
    ./target/release/tentabrowser fetch https://example.com/ --dump markdown
EOF
    if [ -n "$V8_DIR" ]; then
        cat <<EOF

  Commands that build V8 take the bundle from the environment; this sets
  TENTA_V8_ARTIFACT_DIR and, for a bundle built here, TENTA_V8_LOCAL_PIN:
    eval "\$(python3 crates/tenta-v8/native/install_bundle.py env)"
    cargo build --release -p tentabrowser-cli --features full
    cargo nextest run --workspace
EOF
    else
        cat <<EOF

  Without a V8 bundle, leave the V8 packages out of workspace commands:
    cargo nextest run --workspace --exclude tenta-v8 --exclude tenta-js-v8
EOF
    fi
}

detect_platform
setup_system
setup_rust
setup_cargo_tools
setup_v8
report_repository
summary
