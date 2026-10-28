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

PACKAGES=""
detect_packages() {
    local id=""
    if [ -r /etc/os-release ]; then
        id="$( . /etc/os-release; printf '%s' "${ID:-} ${ID_LIKE:-}" )"
    fi
    case "$id" in
        *arch*) PACKAGES="base-devel pkgconf wayland pam fontconfig libxkbcommon vulkan-icd-loader" ;;
        *debian*|*ubuntu*)
            PACKAGES="build-essential pkg-config libwayland-dev libpam0g-dev libfontconfig1-dev libxkbcommon-dev libvulkan-dev" ;;
        *fedora*|*rhel*)
            PACKAGES="gcc gcc-c++ make pkgconf-pkg-config wayland-devel pam-devel fontconfig-devel libxkbcommon-devel vulkan-loader-devel" ;;
        *suse*) PACKAGES="gcc gcc-c++ make pkg-config wayland-devel pam-devel fontconfig-devel libxkbcommon-devel vulkan-devel" ;;
        *) PACKAGES="" ;;
    esac
}

missing_deps() {
    local missing=0
    command -v pkg-config >/dev/null 2>&1 || missing=1
    pkg-config --exists wayland-client 2>/dev/null || missing=1
    pkg-config --exists libpam 2>/dev/null || missing=1
    pkg-config --exists fontconfig 2>/dev/null || missing=1
    pkg-config --exists xkbcommon 2>/dev/null || missing=1
    command -v cc >/dev/null 2>&1 || missing=1
    return $missing
}

install_deps() {
    if ! missing_deps; then
        ui_ok "dependencies" "build toolchain already present"
        return 0
    fi
    if [ "$INSTALL_DEPS" -eq 0 ]; then
        ui_skip "dependencies" "missing (--no-deps)"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "dependencies" "[dry-run] would install: $PACKAGES"
        return 0
    fi
    detect_packages
    if [ -z "$PACKAGES" ]; then
        ui_warn "unknown distribution: install these manually: pkg-config wayland pam fontconfig xkbcommon vulkan"
        return 0
    fi
    local pm=""
    for candidate in pacman apt-get dnf zypper; do
        command -v "$candidate" >/dev/null 2>&1 && { pm="$candidate"; break; }
    done
    [ -n "$pm" ] || { ui_warn "no package manager found; install manually: $PACKAGES"; return 0; }
    case "$pm" in
        pacman)
            run_spin "dependencies" as_root pacman -S --needed --noconfirm $PACKAGES ;;
        apt-get)
            as_root apt-get update >>"$LOG_FILE" 2>&1 || true
            DEBIAN_FRONTEND=noninteractive run_spin "dependencies" as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y $PACKAGES ;;
        dnf)
            run_spin "dependencies" as_root dnf install -y $PACKAGES ;;
        zypper)
            run_spin "dependencies" as_root zypper --non-interactive install $PACKAGES ;;
    esac || die "could not install the build dependencies"
    if missing_deps; then
        ui_warn "some dependencies are still missing: check the log ($LOG_FILE)"
    else
        ui_ok "dependencies" "installed with $pm"
    fi
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
        wait "$pid" || die "cargo build failed"
        printf '\r\033[K'
    else
        ( cd "$SCRIPT_DIR" && nice -n 10 ionice -c2 -n 7 cargo build --release --locked ) >>"$LOG_FILE" 2>&1 \
            || die "cargo build failed"
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

    install_deps
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

LOG_FILE="$(mktemp)"
trap 'rm -f "$LOG_FILE"' EXIT

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
