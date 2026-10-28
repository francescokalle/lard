#!/usr/bin/env bash
#
# LARD installer — one command, dependencies included.
#
#   ./install.sh                 install everything (deps, build, files, autostart)
#   ./install.sh --uninstall     remove binary/PAM/docs (the config stays)
#   ./install.sh --user          user-only install (~/.local/bin, ~/.config)
#   ./install.sh --niri          also add the binds to ~/.config/niri/config.kdl
#   ./install.sh --help          all the options
#
set -Eeuo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="/usr/local"
SYSCONFDIR="/etc"
DESTDIR=""
USER_MODE=0
SKIP_BUILD=0
FORCE=0
INSTALL_DEPS=1
DO_PAM=1
DO_CONFIG=1
DO_DOCS=1
DO_GREETER=1
DO_SYSTEMD=1
DO_ENABLE=1
DO_NIRI=0
DRY_RUN=0
UNINSTALL=0
PURGE=0
ASSUME_YES=0
BINARY_OVERRIDE=""
LOG_FILE=""
UI_TTY=0

# ---------------------------------------------------------------------------
# Output: a small TUI when stdout is a terminal, plain lines otherwise.

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then UI_TTY=1; fi
readonly C_RESET=$'\033[0m' C_DIM=$'\033[2m' C_BOLD=$'\033[1m'
readonly C_GOLD=$'\033[38;5;179m' C_GREEN=$'\033[32m' C_RED=$'\033[31m'
readonly C_BLUE=$'\033[34m' C_GREY=$'\033[90m'

ui_header() {
    printf '\n%s' "$C_GOLD$C_BOLD"
    printf '%s\n' "  ╭──────────────────────────────────────────────╮"
    printf '%s\n' "  │  LARD — light as rust dust                   │"
    printf '%s\n' "  │  native Wayland shell for niri               │"
    printf '%s\n' "  ╰──────────────────────────────────────────────╯"
    printf '%s\n\n' "$C_RESET"
}

ui_step() { printf '  %s◐%s %s%-12s%s %s%s%s\n' "$C_BLUE" "$C_RESET" "$C_BOLD" "$1" "$C_RESET" "$C_GREY" "${2:-}" "$C_RESET"; }
ui_ok()   { printf '  %s✓%s %s%-12s%s %s%s%s\n' "$C_GREEN" "$C_RESET" "$C_BOLD" "$1" "$C_RESET" "$C_GREY" "${2:-}" "$C_RESET"; }
ui_skip() { printf '  %s·%s %s%-12s%s %s%s%s\n' "$C_GREY" "$C_RESET" "$C_BOLD" "$1" "$C_RESET" "$C_GREY" "${2:-}" "$C_RESET"; }
ui_warn() { printf '  %s!%s %s\n' "$C_GOLD" "$C_RESET" "$*" >&2; }
ui_fail() { printf '  %s✗%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; }
die()     { ui_fail "$*"; [ -n "$LOG_FILE" ] && [ -s "$LOG_FILE" ] && { printf '\n%s--- last log lines ---%s\n' "$C_GREY" "$C_RESET" >&2; tail -n 25 "$LOG_FILE" >&2; }; exit 1; }

# Run a command with a spinner, keeping its output in the log.
run_spin() {
    local label="$1"; shift
    if [ "$UI_TTY" -eq 0 ]; then
        "$@" >>"$LOG_FILE" 2>&1
        return $?
    fi
    local frames='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏' i=0 start=$SECONDS pid rc
    "$@" >>"$LOG_FILE" 2>&1 &
    pid=$!
    printf '  %s◐%s %s%-12s%s ' "$C_BLUE" "$C_RESET" "$C_BOLD" "$label" "$C_RESET"
    while kill -0 "$pid" 2>/dev/null; do
        printf '\r  %s%s%s %s%-12s%s %s%s · %ss%s' "$C_BLUE" "${frames:i++%${#frames}:1}" "$C_RESET" \
            "$C_BOLD" "$label" "$C_RESET" "$C_GREY" "$1" "$((SECONDS-start))" "$C_RESET"
        sleep 0.12
    done
    wait "$pid"; rc=$?
    printf '\r\033[K'
    return $rc
}

usage() {
    cat <<'EOF'
LARD installer

USAGE
  ./install.sh [options]

OPTIONS
  --install-deps / --no-deps  install the build dependencies first (default: yes)
  --niri                      also add the binds to ~/.config/niri/config.kdl
  --user                      user-only install: ~/.local/bin and ~/.config
  --root DIR                  packaging staging (DESTDIR); does not use sudo
  --prefix DIR                installation prefix (default /usr/local)
  --sysconfdir DIR            configuration directory (default /etc)
  --skip-build                use an already built binary
  --binary PATH               use a specific binary
  --force                     overwrite existing config/PAM after a backup
  --no-enable                 do not enable autostart
  --no-pam / --no-config / --no-greeter / --no-docs / --no-systemd
  --uninstall                 uninstall
  --purge                     with --uninstall, also remove the config
  --dry-run                   show what would happen, change nothing
  -y, --yes                   do not ask for confirmation on uninstall
  -h, --help                  this help

EXAMPLES
  ./install.sh
  ./install.sh --niri
  ./install.sh --user
  ./install.sh --uninstall
EOF
}

# ---------------------------------------------------------------------------
# Arguments

need_arg() { [ "$#" -ge 2 ] || die "option $1 needs a value"; }
normalize_dir() { local value="${1%/}"; printf '%s' "${value:-/}"; }

while [ "$#" -gt 0 ]; do
    case "$1" in
        --install-deps) INSTALL_DEPS=1 ;;
        --no-deps) INSTALL_DEPS=0 ;;
        --niri) DO_NIRI=1 ;;
        --user) USER_MODE=1 ;;
        --root) need_arg "$@"; DESTDIR="$(normalize_dir "$2")"; shift ;;
        --root=*) DESTDIR="$(normalize_dir "${1#*=}")" ;;
        --prefix) need_arg "$@"; PREFIX="$(normalize_dir "$2")"; shift ;;
        --prefix=*) PREFIX="$(normalize_dir "${1#*=}")" ;;
        --sysconfdir) need_arg "$@"; SYSCONFDIR="$(normalize_dir "$2")"; shift ;;
        --sysconfdir=*) SYSCONFDIR="$(normalize_dir "${1#*=}")" ;;
        --skip-build) SKIP_BUILD=1 ;;
        --binary) need_arg "$@"; BINARY_OVERRIDE="$2"; shift ;;
        --binary=*) BINARY_OVERRIDE="${1#*=}" ;;
        --force) FORCE=1 ;;
        --uninstall) UNINSTALL=1 ;;
        --purge) PURGE=1 ;;
        --dry-run) DRY_RUN=1 ;;
        --no-pam) DO_PAM=0 ;;
        --no-config) DO_CONFIG=0 ;;
        --no-greeter) DO_GREETER=0 ;;
        --no-docs) DO_DOCS=0 ;;
        --no-systemd) DO_SYSTEMD=0 ;;
        --no-enable) DO_ENABLE=0 ;;
        --enable) DO_ENABLE=1 ;;
        -y|--yes) ASSUME_YES=1 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
    shift
done

[ "$PURGE" -eq 1 ] && UNINSTALL=1
if [ "$USER_MODE" -eq 1 ] && [ -n "$DESTDIR" ]; then
    die "--user and --root cannot be used together"
fi
if [ "$USER_MODE" -eq 1 ]; then
    PREFIX="${HOME}/.local"
    DO_PAM=0
    DO_GREETER=0
    DO_SYSTEMD=0
fi

staged() { [ -n "$DESTDIR" ]; }
# In packaging/staging and --user we never need root.
as_root() {
    if staged || [ "$USER_MODE" -eq 1 ] || [ "$EUID" -eq 0 ]; then
        "$@"
    else
        sudo "$@"
    fi
}
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        local joined="" arg
        for arg in "$@"; do joined+="$arg "; done
        printf '  %s[dry-run]%s %s\n' "$C_GREY" "$C_RESET" "${joined% }"
    else
        "$@"
    fi
}

# ---------------------------------------------------------------------------
# Dependencies


# ---------------------------------------------------------------------------
# Helpers used by the dependency step and by failure reporting

sudo_warmup() {
    # `sudo` prompts on the controlling terminal; a backgrounded job (the
    # spinner) cannot read it, so the prompt would fail silently. Ask once,
    # in the foreground, before any spinner starts.
    [ "$DRY_RUN" -eq 1 ] && return 0
    if staged || [ "$USER_MODE" -eq 1 ] || [ "$EUID" -eq 0 ]; then return 0; fi
    command -v sudo >/dev/null 2>&1 || return 0
    if ! sudo -n true >/dev/null 2>&1; then
        printf '  %s·%s %s\n' "$C_GREY" "$C_RESET" "sudo is needed for the system files and packages"
        sudo -v || die "sudo is required to install into $PREFIX and $SYSCONFDIR (or use --user)"
    fi
}

version_ge() { # version_ge HAVE NEED
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" = "$2" ]
}

# smithay-client-toolkit 0.21 generates bindings from wayland-client and
# wayland-protocols at build time: too old a tree fails inside its build script.
protocol_requirements() {
    local client protocols scanner datadir bad=0
    client="$(pkg-config --modversion wayland-client 2>/dev/null || echo 0)"
    protocols="$(pkg-config --modversion wayland-protocols 2>/dev/null || echo 0)"
    scanner="$(wayland-scanner --version 2>/dev/null | awk '{print $NF}' || true)"; scanner="${scanner:-0}"
    datadir="$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null || true)"
    version_ge "$client" "1.20" || { ui_warn "wayland-client $client is too old (need >= 1.20)"; bad=1; }
    version_ge "$scanner" "1.20" || { ui_warn "wayland-scanner $scanner is too old (need >= 1.20)"; bad=1; }
    if ! version_ge "$protocols" "1.31"; then
        ui_warn "wayland-protocols $protocols is too old: smithay-client-toolkit 0.21 needs >= 1.31"
        bad=1
    fi
    if [ -n "$datadir" ]; then
        for xml in stable/ext-session-lock/ext-session-lock-v1.xml \
                   staging/ext-session-lock/ext-session-lock-v1.xml \
                   stable/viewporter/viewporter.xml; do
            [ -f "$datadir/$xml" ] || { ui_warn "missing protocol xml: $datadir/$xml"; bad=1; }
        done
    fi
    if [ "$bad" -eq 1 ]; then
        ui_warn "how to fix:"
        ui_warn "  Arch:    pacman -S --needed wayland wayland-protocols"
        ui_warn "  Debian:  apt install wayland-protocols libwayland-bin (bookworm+)"
        ui_warn "  Fedora:  dnf install wayland-devel wayland-protocols-devel"
        ui_warn "  Ubuntu 22.04/Mint ship wayland-protocols 1.25 (too old): upgrade or build 1.31+ from source"
    fi
    return $bad
}

build_diagnostics() {
    {
        echo "--- diagnostics ---"
        echo "date:    $(date -Is)"
        echo "os:      ${PRETTY_NAME:-unknown}"
        echo "rustc:   $(rustc -V 2>/dev/null || echo missing)"
        echo "cargo:   $(cargo -V 2>/dev/null || echo missing)"
        echo "cc:      $(cc --version 2>/dev/null | head -1 || echo missing)"
        echo "wayland-client:    $(pkg-config --modversion wayland-client 2>/dev/null || echo missing)"
        echo "wayland-protocols: $(pkg-config --modversion wayland-protocols 2>/dev/null || echo missing)"
        echo "wayland-scanner:   $(wayland-scanner --version 2>/dev/null || echo missing)"
        echo "xkbcommon: $(pkg-config --modversion xkbcommon 2>/dev/null || echo missing)"
    } | tee -a "$LOG_FILE" >&2
}

# ---------------------------------------------------------------------------
# Dependencies: detect what is missing, install it with the system package
# manager, then verify it. Nothing here is distro-guessed: every requirement has
# its own probe and its own package name per distro family.

FAMILY=""
detect_family() {
    local id=""
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        id="$( . /etc/os-release; printf '%s %s' "${ID:-}" "${ID_LIKE:-}" )"
    fi
    case "$id" in
        *arch*|*manjaro*|*endeavour*|*cachy*) FAMILY=arch ;;
        *debian*|*ubuntu*|*mint*|*pop*) FAMILY=debian ;;
        *fedora*|*rhel*|*centos*|*rocky*|*alma*) FAMILY=fedora ;;
        *suse*|*opensuse*) FAMILY=suse ;;
        *) FAMILY=unknown ;;
    esac
}

# component → package(s) for the detected family
pkg_for() {
    case "$1:$FAMILY" in
        cc:*) printf '%s\n' gcc ;;
        make:*) printf '%s\n' make ;;
        pkgconf:arch) printf '%s\n' pkgconf ;;
        pkgconf:debian|pkgconf:suse) printf '%s\n' pkg-config ;;
        pkgconf:fedora) printf '%s\n' pkgconf-pkg-config ;;
        wayland:arch) printf '%s\n' wayland wayland-protocols ;;
        wayland:debian) printf '%s\n' libwayland-dev libwayland-bin wayland-protocols ;;
        wayland:fedora|wayland:suse) printf '%s\n' wayland-devel wayland-protocols-devel ;;
        scanner:arch) printf '%s\n' wayland ;;
        scanner:debian) printf '%s\n' libwayland-bin libwayland-dev ;;
        scanner:fedora|scanner:suse) printf '%s\n' wayland-devel ;;
        protocols:arch|protocols:debian) printf '%s\n' wayland-protocols ;;
        protocols:fedora|protocols:suse) printf '%s\n' wayland-protocols-devel ;;
        pam:arch) printf '%s\n' pam ;;
        pam:debian) printf '%s\n' libpam0g-dev ;;
        pam:fedora|pam:suse) printf '%s\n' pam-devel ;;
        fontconfig:arch) printf '%s\n' fontconfig ;;
        fontconfig:debian) printf '%s\n' libfontconfig1-dev ;;
        fontconfig:fedora|fontconfig:suse) printf '%s\n' fontconfig-devel ;;
        xkbcommon:arch) printf '%s\n' libxkbcommon ;;
        xkbcommon:debian) printf '%s\n' libxkbcommon-dev ;;
        xkbcommon:fedora|xkbcommon:suse) printf '%s\n' libxkbcommon-devel ;;
        vulkan:arch) printf '%s\n' vulkan-icd-loader ;;
        vulkan:debian) printf '%s\n' libvulkan-dev ;;
        vulkan:fedora) printf '%s\n' vulkan-loader-devel ;;
        vulkan:suse) printf '%s\n' vulkan-devel ;;
        *) ;;
    esac
}

# Human readable fix for one component.
hint_for() {
    case "$1" in
        cc) echo "C compiler (gcc or clang)" ;;
        make) echo "make" ;;
        pkgconf) echo "pkg-config" ;;
        wayland) echo "wayland client headers" ;;
        scanner) echo "wayland-scanner (protocol code generator)" ;;
        protocols) echo "wayland-protocols >= 1.31 (ext-session-lock-v1)" ;;
        pam) echo "PAM development headers" ;;
        fontconfig) echo "fontconfig development headers" ;;
        xkbcommon) echo "xkbcommon development headers" ;;
        vulkan) echo "Vulkan loader" ;;
        *) echo "$1" ;;
    esac
}

# One line per missing requirement.
missing_components() {
    { command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 \
        || command -v clang >/dev/null 2>&1; } || echo cc
    command -v make >/dev/null 2>&1 || echo make
    command -v pkg-config >/dev/null 2>&1 || echo pkgconf
    pkg-config --exists wayland-client 2>/dev/null || echo wayland
    command -v wayland-scanner >/dev/null 2>&1 || echo scanner
    pkg-config --exists wayland-protocols 2>/dev/null || echo protocols
    pkg-config --exists libpam 2>/dev/null || echo pam
    pkg-config --exists fontconfig 2>/dev/null || echo fontconfig
    pkg-config --exists xkbcommon 2>/dev/null || echo xkbcommon
    pkg-config --exists vulkan 2>/dev/null || echo vulkan
}

missing_deps() { [ -n "$(missing_components)" ]; }

package_manager() {
    for candidate in pacman apt-get dnf zypper; do
        command -v "$candidate" >/dev/null 2>&1 && { printf '%s' "$candidate"; return 0; }
    done
    printf ''
}

install_deps() {
    detect_family
    local missing
    missing="$(missing_components)"
    if [ -z "$missing" ]; then
        ui_ok "dependencies" "everything already installed"
        return 0
    fi
    if [ "$INSTALL_DEPS" -eq 0 ]; then
        ui_skip "dependencies" "missing (--no-deps): $(echo "$missing" | tr '\n' ' ')"
        return 0
    fi
    # The list is built from what is actually missing, per component.
    local -a pkgs=()
    local component
    while IFS= read -r component; do
        [ -n "$component" ] || continue
        while IFS= read -r pkg; do
            [ -n "$pkg" ] && pkgs+=("$pkg")
        done < <(pkg_for "$component")
    done <<< "$missing"
    if [ "${#pkgs[@]}" -eq 0 ]; then
        ui_warn "unknown distribution: install manually: $(echo "$missing" | tr '\n' ' ')"
        return 0
    fi
    # shellcheck disable=SC2207
    local -a unique=($(printf '%s\n' "${pkgs[@]}" | sort -u))
    pkgs=("${unique[@]}")
    local packages
    packages="$(printf '%s ' "${pkgs[@]}")"
    packages="${packages% }"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "dependencies" "[dry-run] would install: $packages"
        return 0
    fi
    local pm; pm="$(package_manager)"
    [ -n "$pm" ] || { ui_warn "no package manager found; install manually: $packages"; return 0; }
    ui_step "dependencies" "installing: $packages"
    sudo_warmup
    case "$pm" in
        pacman)
            # A stale/empty sync database answers "target not found" for packages
            # that certainly exist: refresh it first, then install.
            if ! pacman -Si "${pkgs[@]}" >/dev/null 2>&1; then
                ui_step "dependencies" "refreshing the pacman database"
                as_root pacman -Sy --noconfirm >>"$LOG_FILE" 2>&1 || true
            fi
            run_spin "dependencies" as_root pacman -S --needed --noconfirm "${pkgs[@]}" ;;
        apt-get)
            as_root apt-get update >>"$LOG_FILE" 2>&1 || true
            run_spin "dependencies" as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${pkgs[@]}" ;;
        dnf) run_spin "dependencies" as_root dnf install -y "${pkgs[@]}" ;;
        zypper) run_spin "dependencies" as_root zypper --non-interactive install "${pkgs[@]}" ;;
    esac || {
        ui_fail "could not install: $packages"
        if grep -qi "target not found" "$LOG_FILE"; then
            ui_warn "the package database does not know these packages: refresh it with"
            ui_warn "  sudo pacman -Syu   (or apt-get update / dnf makecache)"
            ui_warn "then run the installer again"
        fi
        grep -iE "error|not found|failed|no such" "$LOG_FILE" | tail -8 >&2 || true
        die "dependency installation failed (full log: $LOG_FILE)"
    }
}

# rustc links with `cc`; when only `gcc`/`clang` exist we point it there instead
# of failing with "linker `cc` not found".
ensure_cc() {
    if command -v cc >/dev/null 2>&1; then return 0; fi
    local alt=""
    for candidate in gcc clang; do
        command -v "$candidate" >/dev/null 2>&1 && { alt="$candidate"; break; }
    done
    if [ -n "$alt" ]; then
        export CC="$alt"
        export RUSTFLAGS="${RUSTFLAGS:-} -C linker=$alt"
        ui_ok "linker" "using $alt (no cc)"
        return 0
    fi
    ui_fail "no C compiler/linker found (need gcc or clang)"
    ui_warn "install it and re-run:  pacman -S base-devel | apt install build-essential | dnf install gcc"
    die "cannot build without a C toolchain"
}

ensure_rust() {
    if command -v cargo >/dev/null 2>&1; then
        ui_ok "rust" "$(cargo --version 2>/dev/null | head -1)"
        return 0
    fi
    if [ "$INSTALL_DEPS" -eq 0 ]; then
        die "cargo not found (needed to build; drop --no-deps)"
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "rust" "[dry-run] would install the stable toolchain with rustup"
        return 0
    fi
    command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 \
        || die "curl or wget is needed to install Rust"
    run_spin "rust" bash -c 'curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path' \
        || die "rustup failed"
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    export PATH="$HOME/.cargo/bin:$PATH"
    ui_ok "rust" "$(cargo --version 2>/dev/null | head -1)"
}

# ---------------------------------------------------------------------------
# Files

install_keep() { # src dst mode label
    local src="$1" dst="$2" mode="$3" label="$4"
    if [ -e "$dst" ] && [ "$FORCE" -eq 0 ]; then
        ui_skip "$label" "already present, kept: $dst"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "$label" "[dry-run] would install $dst"
        return 0
    fi
    [ -e "$dst" ] && [ "$FORCE" -eq 1 ] && as_root cp -a "$dst" "$dst.bak-$(date +%Y%m%d%H%M%S)"
    as_root install -Dm"$mode" "$src" "$dst"
    ui_ok "$label" "$dst"
}

user_config_path() {
    printf '%s/lard/config.toml' "${XDG_CONFIG_HOME:-$HOME/.config}"
}

# A cargo failure is a wall of "due to a previous error": show the roots only.
build_failed() {
    ui_fail "cargo build failed"
    if [ -s "$LOG_FILE" ]; then
        printf '  %s--- errors ---%s\n' "$C_GREY" "$C_RESET" >&2
        grep -E "error(\[E[0-9]+\])?:|linker|not found|cannot find|could not find|No package" "$LOG_FILE" \
            | grep -v "due to a previous error" | head -12 >&2 || true
        # A failed "custom build command" (smithay-client-toolkit, wayland-sys,
        # xkbcommon…) hides its reason behind a "--- stderr" block.
        if grep -q "failed to run custom build command" "$LOG_FILE"; then
            printf '  %s--- build script output ---%s\n' "$C_GREY" "$C_RESET" >&2
            sed -n '/--- stderr/,/^$/p' "$LOG_FILE" | head -16 >&2 || true
            grep -q "wayland-scanner" "$LOG_FILE" \
                && ui_warn "wayland-scanner missing: install libwayland-bin (Debian) or wayland (Arch)"
        fi
    fi
    build_diagnostics
    ui_warn "full log: $LOG_FILE"
    if grep -q "linker .cc. not found\|linker .cc. not found" "$LOG_FILE" 2>/dev/null; then
        ui_warn "the C linker is missing: install gcc (pacman -S base-devel | apt install build-essential | dnf install gcc)"
    fi
    exit 1
}

BIN_SRC=""
build_binary() {
    local out="$SCRIPT_DIR/target/release/lard"
    BIN_SRC="$out"
    if [ -n "$BINARY_OVERRIDE" ]; then
        [ -x "$BINARY_OVERRIDE" ] || die "binary not executable: $BINARY_OVERRIDE"
        BIN_SRC="$BINARY_OVERRIDE"
        return 0
    fi
    if [ "$SKIP_BUILD" -eq 1 ]; then
        [ -x "$out" ] || die "--skip-build requested but $out does not exist"
        return 0
    fi
    [ -f "$SCRIPT_DIR/Cargo.toml" ] || die "no Cargo.toml in $SCRIPT_DIR"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "build" "[dry-run] would build target/release/lard"
        return 0
    fi
    local started=$SECONDS
    if [ "$UI_TTY" -eq 1 ]; then
        ( cd "$SCRIPT_DIR" && nice -n 10 ionice -c2 -n 7 cargo build --release --locked ) >>"$LOG_FILE" 2>&1 &
        local pid=$! frames='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏' i=0
        while kill -0 "$pid" 2>/dev/null; do
            local crates
            crates="$(grep -c '^\s*Compiling' "$LOG_FILE" 2>/dev/null || true)"
            printf '\r  %s%s%s %s%-12s%s %sbuilding · %s crates · %ss%s' "$C_BLUE" \
                "${frames:i++%${#frames}:1}" "$C_RESET" "$C_BOLD" "build" "$C_RESET" \
                "$C_GREY" "${crates:-0}" "$((SECONDS-started))" "$C_RESET"
            sleep 0.15
        done
        wait "$pid" || build_failed
        printf '\r\033[K'
    else
        ( cd "$SCRIPT_DIR" && nice -n 10 ionice -c2 -n 7 cargo build --release --locked ) >>"$LOG_FILE" 2>&1 \
            || build_failed
    fi
    [ -x "$out" ] || die "build finished but $out does not exist"
    ui_ok "build" "release · $((SECONDS-started))s"
}

# ---------------------------------------------------------------------------
# Autostart

enable_autostart() {
    if [ "$DO_ENABLE" -eq 0 ] || staged || [ "$USER_MODE" -eq 1 ]; then
        ui_skip "autostart" "not requested"
        return 0
    fi
    local niri_cfg="${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl"
    if command -v systemctl >/dev/null 2>&1 \
        && [ "$(systemctl --user is-enabled lard.service 2>/dev/null || true)" = "enabled" ]; then
        ui_ok "autostart" "lard.service (user systemd) already enabled"
        return 0
    fi
    if [ -f "$niri_cfg" ] && grep -Eq '^[[:space:]]*spawn-at-startup[^/]*lard' "$niri_cfg"; then
        ui_ok "autostart" "already configured in $niri_cfg"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "autostart" "[dry-run] would enable lard.service"
        return 0
    fi
    if command -v systemctl >/dev/null 2>&1 \
        && systemctl --user daemon-reload >/dev/null 2>&1 \
        && systemctl --user enable --now lard.service >/dev/null 2>&1; then
        ui_ok "autostart" "lard.service (user systemd) enabled"
        return 0
    fi
    if [ -f "$niri_cfg" ]; then
        cp -a "$niri_cfg" "$niri_cfg.bak-$(date +%Y%m%d%H%M%S)"
        printf '\n// >>> lard (autostart, managed by install.sh)\nspawn-at-startup "%s/bin/lard"\n// <<< lard\n' \
            "$PREFIX" >> "$niri_cfg"
        ui_ok "autostart" "spawn-at-startup added to $niri_cfg"
        return 0
    fi
    ui_warn "autostart not configured: enable lard.service or add spawn-at-startup \"$PREFIX/bin/lard\""
}

# ---------------------------------------------------------------------------
# Uninstall

do_uninstall() {
    local bin_dst doc_dst etc_dir pam_file unit_file
    if [ "$USER_MODE" -eq 1 ]; then
        bin_dst="$HOME/.local/bin/lard"; doc_dst="$HOME/.local/share/doc/lard"
        etc_dir=""; pam_file=""; unit_file=""
    else
        bin_dst="$DESTDIR$PREFIX/bin/lard"; doc_dst="$DESTDIR$PREFIX/share/doc/lard"
        etc_dir="$DESTDIR$SYSCONFDIR/lard"; pam_file="$DESTDIR$SYSCONFDIR/pam.d/lard-greeter"
        unit_file="$DESTDIR$PREFIX/lib/systemd/user/lard.service"
    fi
    if [ "$ASSUME_YES" -ne 1 ] && [ "$DRY_RUN" -eq 0 ]; then
        [ -t 0 ] || die "non-interactive uninstall: add --yes"
        local answer
        read -r -p "  Remove LARD from this system? [y/N] " answer
        case "$answer" in y|Y|yes|YES) ;; *) die "cancelled" ;; esac
    fi
    if [ -n "$unit_file" ] && [ "$DRY_RUN" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now lard.service >/dev/null 2>&1 || true
    fi
    for path in "$bin_dst" "$doc_dst" "$pam_file" "$unit_file" "$etc_dir/greeter-sway.conf" "$etc_dir/greeter.kdl"; do
        [ -n "$path" ] && [ -e "$path" ] || continue
        if staged || [ "$USER_MODE" -eq 1 ] || [ "$EUID" -eq 0 ]; then rm -rf -- "$path"; else sudo rm -rf -- "$path"; fi
        printf '  %s·%s %s\n' "$C_GREY" "$C_RESET" "removed $path"
    done
    if [ "$PURGE" -eq 1 ]; then
        [ -n "$etc_dir" ] && [ -e "$etc_dir" ] && { staged || [ "$EUID" -eq 0 ] && rm -rf "$etc_dir" || sudo rm -rf "$etc_dir"; }
        local ucfg; ucfg="$(user_config_path)"
        [ -e "$ucfg" ] && rm -f "$ucfg" && printf '  %s·%s removed %s\n' "$C_GREY" "$C_RESET" "$ucfg"
    fi
    printf '\n  %s✓%s LARD removed%s\n\n' "$C_GREEN" "$C_RESET" ""
}

# ---------------------------------------------------------------------------
# Install

do_install() {
    local bin_dst doc_dst etc_dir pam_file unit_file
    bin_dst="$DESTDIR$PREFIX/bin/lard"
    doc_dst="$DESTDIR$PREFIX/share/doc/lard"
    etc_dir="$DESTDIR$SYSCONFDIR/lard"
    pam_file="$DESTDIR$SYSCONFDIR/pam.d/lard-greeter"
    unit_file="$DESTDIR$PREFIX/lib/systemd/user/lard.service"

    ui_header
    printf '  %s%s%s\n' "$C_GREY" "prefix $PREFIX · config $SYSCONFDIR${DESTDIR:+ · staging $DESTDIR}" "$C_RESET"

    sudo_warmup
    install_deps
    protocol_requirements || ui_warn "the wayland stack is older than smithay-client-toolkit 0.21 expects"
    ensure_cc
    [ "$SKIP_BUILD" -eq 1 ] || ensure_rust

    build_binary
    local bin_src="$BIN_SRC"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "install" "[dry-run] would install $bin_dst"
    else
        as_root install -Dm755 "$bin_src" "$bin_dst"
        ui_ok "install" "$bin_dst"
    fi

    if [ "$DO_CONFIG" -eq 1 ]; then
        install_keep "$SCRIPT_DIR/config/config.toml" "$DESTDIR$SYSCONFDIR/lard/config.toml" 644 "config"
        local ucfg; ucfg="$(user_config_path)"
        if staged; then
            ui_skip "user config" "skipped in staging"
        elif [ -e "$ucfg" ]; then
            ui_skip "user config" "kept: $ucfg"
        elif [ "$DRY_RUN" -eq 1 ]; then
            ui_skip "user config" "[dry-run] would create $ucfg"
        else
            install -Dm644 "$SCRIPT_DIR/config/config.toml" "$ucfg"
            ui_ok "user config" "$ucfg"
        fi
    fi

    if [ "$DO_PAM" -eq 1 ]; then
        install_keep "$SCRIPT_DIR/docs/pam.d/lard-greeter" "$pam_file" 644 "pam"
    fi

    if [ "$DO_GREETER" -eq 1 ]; then
        install_keep "$SCRIPT_DIR/docs/greetd/sway-greeter.conf" "$etc_dir/greeter-sway.conf" 644 "greeter"
        install_keep "$SCRIPT_DIR/docs/greeter-lard.kdl" "$etc_dir/greeter.kdl" 644 "greeter"
    fi

    if [ "$DO_SYSTEMD" -eq 1 ]; then
        local unit_tmp; unit_tmp="$(mktemp)"
        sed "s|/usr/local/bin/lard|$PREFIX/bin/lard|" \
            "$SCRIPT_DIR/docs/systemd/lard.service" > "$unit_tmp"
        install_keep "$unit_tmp" "$unit_file" 644 "systemd"
        rm -f "$unit_tmp"
    fi

    if [ "$DO_NIRI" -eq 1 ] && ! staged && [ "$EUID" -ne 0 ]; then
        local cfg="${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl"
        if [ -f "$cfg" ] && grep -q ">>> lard" "$cfg"; then
            ui_skip "niri" "binds already present"
        else
            if [ -f "$cfg" ]; then cp -a "$cfg" "$cfg.bak-$(date +%Y%m%d%H%M%S)"; fi
            cat >> "$cfg" <<EOF

// >>> lard (managed by install.sh)
spawn-at-startup "$PREFIX/bin/lard"

binds {
    Mod+D       { spawn "$PREFIX/bin/lard" "ipc" "launcher"; }
    Mod+Shift+E { spawn "$PREFIX/bin/lard" "ipc" "control-center"; }
    Mod+Escape  { spawn "$PREFIX/bin/lard" "lock"; }
    Mod+Shift+Q { spawn "$PREFIX/bin/lard" "ipc" "quit"; }
}
// <<< lard
EOF
            ui_ok "niri" "binds added to $cfg"
        fi
    fi

    if [ "$DO_DOCS" -eq 1 ] && [ "$DRY_RUN" -eq 0 ]; then
        as_root install -d "$doc_dst"
        as_root cp -a "$SCRIPT_DIR/docs/." "$doc_dst/"
        [ -d "$SCRIPT_DIR/preview" ] && as_root cp -a "$SCRIPT_DIR/preview" "$doc_dst/"
        as_root install -m644 "$SCRIPT_DIR/README.md" "$doc_dst/README.md"
        as_root install -m644 "$SCRIPT_DIR/LICENSE" "$doc_dst/LICENSE"
        ui_ok "docs" "$doc_dst"
    fi

    enable_autostart

    if [ "$DRY_RUN" -eq 0 ] && [ -x "$PREFIX/bin/lard" ]; then
        if "$PREFIX/bin/lard" version >/dev/null 2>&1; then
            ui_ok "verify" "$("$PREFIX/bin/lard" version)"
        else
            ui_warn "the binary is installed but does not start here (no Wayland?)"
        fi
    fi

    if [ "$DRY_RUN" -eq 1 ]; then
        printf '\n  %s· dry run: nothing was changed%s\n\n' "$C_GREY" "$C_RESET"
        return 0
    fi
    printf '\n  %s✓ LARD installed%s\n' "$C_GREEN$C_BOLD" "$C_RESET"
    printf '  %sstart it now:%s  %s/bin/lard\n' "$C_GREY" "$C_RESET" "$PREFIX"
    printf '  %sor:%s            systemctl --user restart lard.service\n' "$C_GREY" "$C_RESET"
    printf '  %slockscreen:%s    lard lock   (PAM service installed)\n\n' "$C_GREY" "$C_RESET"
}

# ---------------------------------------------------------------------------

CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/lard"
mkdir -p "$CACHE_DIR" 2>/dev/null || true
LOG_FILE="$CACHE_DIR/install.log"
: > "$LOG_FILE" 2>/dev/null || LOG_FILE="$(mktemp)"

if [ "$UNINSTALL" -eq 1 ]; then
    ui_header
    do_uninstall
    exit 0
fi

if [ "$EUID" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && ! staged; then
    die "run ./install.sh as a normal user ($SUDO_USER), not with sudo: it asks for sudo only for system files"
fi
if ! staged && [ "$USER_MODE" -eq 0 ] && [ "$EUID" -ne 0 ] && ! command -v sudo >/dev/null 2>&1; then
    die "sudo is needed for $PREFIX and $SYSCONFDIR; use --user for a local install"
fi

do_install
