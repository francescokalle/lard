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
TERMINAL=""
TERMINAL_ASKED=0
DO_EXTRAS=1
DO_SETUP=1
REPAIR=0
UPDATE=0
BINARY_OVERRIDE=""
LOG_FILE=""
UI_TTY=0
# Bump with every installer change: the first line of the TUI shows it, so a
# stale downloaded copy is obvious at a glance.
LARD_INSTALLER_VERSION="2026-10-29.2"

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
    printf '  %sinstaller %s%s\n\n' "$C_GREY" "$LARD_INSTALLER_VERSION" "$C_RESET"
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
  --terminal NAME             terminal to install (kitty, foot, alacritty, wezterm,
                              gnome-terminal, xterm, none); if omitted, asks
  --no-extras                 do not install the optional tools (clipboard, brightness)
  --no-system-setup           do not configure portals/audio/services/autostart beyond LARD
  --repair                    fix an existing installation (keeps the configs)
  --update                    non-interactive update: keep the user config and the
                              boot choice, replace the managed files, restart the bar
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
        --terminal) need_arg "$@"; TERMINAL="$2"; TERMINAL_ASKED=1; shift ;;
        --terminal=*) TERMINAL="${1#*=}"; TERMINAL_ASKED=1 ;;
        --no-extras) DO_EXTRAS=0 ;;
        --no-system-setup) DO_SETUP=0 ;;
        --repair) REPAIR=1; FORCE=1 ;;
        --update) UPDATE=1; REPAIR=1; FORCE=1; ASSUME_YES=1 ;;
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

build_diagnostics() {
    {
        echo "--- diagnostics ---"
        echo "date:    $(date -Is)"
        echo "os:      ${PRETTY_NAME:-unknown}"
        echo "rustc:   $(rustc -V 2>/dev/null || echo missing)"
        echo "cargo:   $(cargo -V 2>/dev/null || echo missing)"
        echo "cc:      $(cc --version 2>/dev/null | head -1 || echo missing)"
        echo "wayland-client:    $(pkg-config --modversion wayland-client 2>/dev/null || echo missing)"
        echo "xkbcommon:         $(pkg-config --modversion xkbcommon 2>/dev/null || echo missing)"
        echo "libpam:            $(pkg-config --modversion libpam 2>/dev/null || echo missing)"
        echo "fontconfig:        $(pkg-config --modversion fontconfig 2>/dev/null || echo missing)"
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
# Runtime packages: the compositor, the terminal and the small tools the shell
# calls into. `niri` is required; the rest is asked once.

# package name for a runtime component in the detected family
runtime_pkg() {
    case "$1:$FAMILY" in
        niri:*) printf '%s\n' niri ;;
        kitty:*) printf '%s\n' kitty ;;
        foot:*) printf '%s\n' foot ;;
        alacritty:*) printf '%s\n' alacritty ;;
        wezterm:arch) printf '%s\n' wezterm ;;
        wezterm:debian) printf '%s\n' wezterm ;;
        wezterm:fedora) printf '%s\n' wezterm ;;
        wezterm:suse) printf '%s\n' wezterm ;;
        gnome-terminal:*) printf '%s\n' gnome-terminal ;;
        xterm:*) printf '%s\n' xterm ;;
        wl-clipboard:arch) printf '%s\n' wl-clipboard ;;
        wl-clipboard:debian) printf '%s\n' wl-clipboard ;;
        wl-clipboard:fedora) printf '%s\n' wl-clipboard ;;
        wl-clipboard:suse) printf '%s\n' wl-clipboard ;;
        brightnessctl:arch) printf '%s\n' brightnessctl ;;
        brightnessctl:debian) printf '%s\n' brightnessctl ;;
        brightnessctl:fedora) printf '%s\n' brightnessctl ;;
        brightnessctl:suse) printf '%s\n' brightnessctl ;;
        awww:*) printf '%s\n' awww ;;
        *) ;;
    esac
}

component_present() {
    case "$1" in
        niri) command -v niri >/dev/null 2>&1 ;;
        kitty|foot|alacritty|wezterm|gnome-terminal|xterm) command -v "$1" >/dev/null 2>&1 ;;
        wl-clipboard) command -v wl-copy >/dev/null 2>&1 ;;
        brightnessctl) command -v brightnessctl >/dev/null 2>&1 ;;
        awww) command -v awww >/dev/null 2>&1 ;;
        *) return 0 ;;
    esac
}

# Interactive menu; reads /dev/tty because with `curl | bash` stdin is the script.
ask_terminal() {
    [ "$TERMINAL_ASKED" -eq 1 ] && return 0
    [ "$DRY_RUN" -eq 1 ] && { TERMINAL="kitty"; return 0; }
    if [ ! -r /dev/tty ]; then
        TERMINAL="kitty"
        return 0
    fi
    printf '\n  %s?%s %s\n' "$C_GOLD" "$C_RESET" "Which terminal should be installed?"
    printf '     %s1)%s kitty (default)   %s2)%s foot   %s3)%s alacritty\n' \
        "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET"
    printf '     %s4)%s wezterm          %s5)%s GNOME Terminal   %s6)%s xterm\n' \
        "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET"
    printf '     %s0)%s none\n' "$C_BOLD" "$C_RESET"
    printf '  %s[1]%s ' "$C_GREY" "$C_RESET"
    local answer=""
    read -r answer < /dev/tty || answer=""
    case "${answer:-1}" in
        0) TERMINAL="" ;;
        2) TERMINAL=foot ;;
        3) TERMINAL=alacritty ;;
        4) TERMINAL=wezterm ;;
        5) TERMINAL=gnome-terminal ;;
        6) TERMINAL=xterm ;;
        *) TERMINAL=kitty ;;
    esac
    TERMINAL_ASKED=1
    [ -n "$TERMINAL" ] && ui_ok "terminal" "$TERMINAL"
    return 0
}

# A fresh install often has no system fonts at all: every app shows boxes.
install_fonts() {
    detect_family
    local count=0
    command -v fc-list >/dev/null 2>&1 && count="$(fc-list 2>/dev/null | wc -l)"
    if [ "$count" -gt 0 ]; then
        ui_ok "fonts" "$count fonts available"
        return 0
    fi
    local -a pkgs=()
    case "$FAMILY" in
        arch) pkgs=(ttf-dejavu noto-fonts ttf-liberation) ;;
        debian) pkgs=(fonts-dejavu-core fonts-noto-core fonts-liberation) ;;
        fedora) pkgs=(dejavu-sans-fonts google-noto-sans-fonts liberation-sans-fonts) ;;
        suse) pkgs=(dejavu-fonts noto-sans-fonts liberation-fonts) ;;
        *) pkgs=() ;;
    esac
    if [ "${#pkgs[@]}" -eq 0 ]; then
        ui_warn "no system fonts found: install dejavu/noto by hand"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        local joined
        joined="$(printf '%s ' "${pkgs[@]}")"
        ui_skip "fonts" "[dry-run] would install: ${joined% }"
        return 0
    fi
    local joined
    joined="$(printf '%s ' "${pkgs[@]}")"
    ui_step "fonts" "installing: ${joined% }"
    sudo_warmup
    case "$(package_manager)" in
        pacman) as_root pacman -S --needed --noconfirm "${pkgs[@]}" >>"$LOG_FILE" 2>&1 ;;
        apt-get) as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${pkgs[@]}" >>"$LOG_FILE" 2>&1 ;;
        dnf) as_root dnf install -y "${pkgs[@]}" >>"$LOG_FILE" 2>&1 ;;
        zypper) as_root zypper --non-interactive install "${pkgs[@]}" >>"$LOG_FILE" 2>&1 ;;
    esac || { ui_warn "could not install the fonts: ${pkgs[*]}"; return 0; }
    command -v fc-cache >/dev/null 2>&1 && fc-cache -f >/dev/null 2>&1
    count="$(fc-list 2>/dev/null | wc -l)"
    ui_ok "fonts" "$count fonts available"
}

# awww is an swww fork: on Arch it lives in the AUR, elsewhere it is usually
# absent. Try the AUR helper, then the packaged swww plus a thin wrapper with the
# same command line, so the wallpaper picker works either way.
install_wallpaper_daemon() {
    if have awww; then
        ui_ok "wallpaper" "awww already installed"
        return 0
    fi
    local aur=""
    for helper in paru yay pikaur; do
        command -v "$helper" >/dev/null 2>&1 && { aur="$helper"; break; }
    done
    if [ -n "$aur" ] && [ "$(package_manager)" = pacman ] && [ "$DRY_RUN" -eq 0 ]; then
        ui_step "wallpaper" "installing awww from the AUR with $aur"
        sudo_warmup
        if run_spin "wallpaper" "$aur" -S --needed --noconfirm awww; then
            have awww && { ui_ok "wallpaper" "awww installed"; return 0; }
        fi
    fi
    if pm_install swww; then
        if [ "$DRY_RUN" -eq 0 ]; then
            local tmp; tmp="$(mktemp)"
            printf '#!/bin/sh\nexec swww "$@"\n' > "$tmp"
            as_root install -Dm755 "$tmp" /usr/local/bin/awww
            printf '#!/bin/sh\nexec swww-daemon "$@"\n' > "$tmp"
            as_root install -Dm755 "$tmp" /usr/local/bin/awww-daemon
            rm -f "$tmp"
        fi
        ui_ok "wallpaper" "swww installed (awww wrapper, same command line)"
        return 0
    fi
    ui_warn "no wallpaper daemon: install awww (AUR) or swww; the picker will stay off"
    return 0
}

# The shortcuts used on the development machine, shipped with the sources and
# installed on every target (with a backup and without duplicating the include).
# Install a managed file, backing it up only when its content actually changes:
# an update must not leave a trail of identical .bak files.
install_managed() { # src dst mode
    local src="$1" dst="$2" mode="$3"
    if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
        return 0
    fi
    [ -f "$dst" ] && cp -a "$dst" "$dst.bak-$(date +%Y%m%d%H%M%S)"
    install -Dm"$mode" "$src" "$dst"
}

install_terminal_config() {
    local term
    term="$(basename "${TERMINAL:-kitty}")"
    if [ "$term" != "kitty" ]; then
        ui_skip "terminal" "$term has no managed config"
        return 0
    fi
    [ -n "$DESTDIR" ] && return 0
    local dir="${XDG_CONFIG_HOME:-$HOME/.config}/kitty"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "terminal" "[dry-run] would install kitty.conf (opacity 0.5, blur) + tabs/theme"
        return 0
    fi
    mkdir -p "$dir"
    local f name
    for f in "$SCRIPT_DIR"/config/kitty/*.conf; do
        [ -f "$f" ] || continue
        name="$(basename "$f")"
        install_managed "$f" "$dir/$name" 644
    done
    ui_ok "terminal" "kitty.conf installed (semi-transparent, blur, tabs)"
}

# The wallpaper folder used to be called omarchy-themes. Rename it (and the
# niri includes that point at it) so an update does not leave the old name
# behind. The shell also accepts the legacy path, so a half-finished migration
# never leaves an empty picker.
migrate_wallpaper_dir() {
    local base="${XDG_CONFIG_HOME:-$HOME/.config}"
    local old="$base/niri/omarchy-themes" new="$base/niri/lard-themes"
    local cfg
    if [ -d "$old" ] && [ ! -d "$new" ]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            ui_skip "wallpapers" "[dry-run] would rename omarchy-themes to lard-themes"
        elif mv "$old" "$new"; then
            ui_ok "wallpapers" "renamed ~/.config/niri/omarchy-themes -> lard-themes"
        else
            ui_warn "cannot rename $old to $new"
        fi
    elif [ -d "$old" ] && [ -d "$new" ]; then
        ui_skip "wallpapers" "both omarchy-themes and lard-themes exist: left as is"
    fi
    for cfg in "$base/niri/config.kdl" "$base/niri/config-lard.kdl"; do
        [ -f "$cfg" ] || continue
        grep -q 'omarchy-themes/' "$cfg" 2>/dev/null || continue
        if [ "$DRY_RUN" -eq 1 ]; then
            ui_skip "wallpapers" "[dry-run] would point $cfg at lard-themes/"
            continue
        fi
        cp -a "$cfg" "$cfg.bak-$(date +%Y%m%d%H%M%S)"
        sed -i 's|omarchy-themes/|lard-themes/|g' "$cfg" \
            && ui_ok "wallpapers" "includes updated in $cfg" \
            || ui_warn "cannot update the includes in $cfg"
    done
}

install_niri_config() {
    [ -n "$DESTDIR" ] && { ui_skip "niri" "skipped in staging"; return 0; }
    [ "$EUID" -eq 0 ] && { ui_skip "niri" "run as your user"; return 0; }
    local dir cfg plain drop src="$SCRIPT_DIR/config/niri"
    dir="${XDG_CONFIG_HOME:-$HOME/.config}/niri"
    cfg="$dir/config-lard.kdl"
    plain="$dir/config.kdl"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "niri" "[dry-run] would install config-lard.kdl + lard/*.kdl and the niri.service drop-in"
        return 0
    fi
    mkdir -p "$dir/lard"
    local f name
    for f in "$src"/lard/*.kdl; do
        [ -f "$f" ] || continue
        name="$(basename "$f")"
        install_managed "$f" "$dir/lard/$name" 644
    done
    if [ -n "${TERMINAL:-}" ] && [ "$TERMINAL" != "kitty" ]; then
        sed -i "s|spawn \"kitty\"|spawn \"$TERMINAL\"|g" "$dir/lard/binds.kdl"
    fi
    install_managed "$src/config-lard.kdl" "$cfg" 644
    # A config error here is a black screen after login: refuse to activate it.
    if have niri && ! niri validate -c "$cfg" >>"$LOG_FILE" 2>&1; then
        mv -f "$cfg" "$cfg.invalid"
        ui_warn "niri rejected the generated config: keeping your current one"
        return 0
    fi
    if [ ! -f "$plain" ]; then
        install -Dm644 "$cfg" "$plain"
    fi
    drop="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/niri.service.d/10-lard.conf"
    mkdir -p "$(dirname "$drop")"
    printf '[Service]\nEnvironment=NIRI_CONFIG=%s\n' "$cfg" > "$drop"
    command -v systemctl >/dev/null 2>&1 && systemctl --user daemon-reload >/dev/null 2>&1 || true
    ui_ok "niri" "config-lard.kdl + lard/*.kdl installed (NIRI_CONFIG)"
}

# Install a package only when its command/unit is missing.
have() { command -v "$1" >/dev/null 2>&1; }

pm_install() {
    local -a pkgs=("$@")
    [ "${#pkgs[@]}" -gt 0 ] || return 0
    local joined
    joined="$(printf '%s ' "${pkgs[@]}")"
    joined="${joined% }"
    [ "$DRY_RUN" -eq 1 ] && { ui_skip "packages" "[dry-run] would install: $joined"; return 0; }
    sudo_warmup
    case "$(package_manager)" in
        pacman) run_spin "packages" as_root pacman -S --needed --noconfirm "${pkgs[@]}" ;;
        apt-get) as_root apt-get update >>"$LOG_FILE" 2>&1 || true
                run_spin "packages" as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${pkgs[@]}" ;;
        dnf) run_spin "packages" as_root dnf install -y "${pkgs[@]}" ;;
        zypper) run_spin "packages" as_root zypper --non-interactive install "${pkgs[@]}" ;;
        *) ui_warn "no package manager for: ${pkgs[*]}"; return 1 ;;
    esac
}

# Add a line to niri's config once (with a backup).
# The session config actually in use: NIRI_CONFIG from our niri.service drop-in
# when present, otherwise the plain ~/.config/niri/config.kdl.
niri_config_path() {
    local base="${XDG_CONFIG_HOME:-$HOME/.config}"
    local drop="$base/systemd/user/niri.service.d/10-lard.conf"
    if [ -f "$drop" ]; then
        local custom
        custom="$(sed -n 's/^Environment=NIRI_CONFIG=//p' "$drop" | head -n1)"
        [ -n "$custom" ] && { printf '%s' "$custom"; return 0; }
    fi
    printf '%s' "$base/niri/config.kdl"
}

niri_spawn() { # niri_spawn "<command line>"
    local cfg line="$1"
    cfg="$(niri_config_path)"
    [ -f "$cfg" ] || return 0
    grep -Fq "$line" "$cfg" && return 0
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "autostart" "[dry-run] would add: $line"
        return 0
    fi
    cp -a "$cfg" "$cfg.bak-$(date +%Y%m%d%H%M%S)"
    printf '\n// >>> lard\n%s\n// <<< lard\n' "$line" >> "$cfg"
}

# Everything that makes a fresh system usable with LARD: fonts (already done),
# portals, audio, the polkit agent, network/bluetooth services and the session
# autostart of the helper daemons.
setup_system() {
    if [ "$DO_SETUP" -eq 0 ] || staged || [ "$USER_MODE" -eq 1 ]; then
        ui_skip "system" "not requested"
        return 0
    fi
    detect_family

    # 1) XDG portals: needed by browsers, file pickers, the color scheme.
    local -a portals=()
    case "$FAMILY" in
        arch) portals=(xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr) ;;
        debian) portals=(xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr) ;;
        fedora) portals=(xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr) ;;
        suse) portals=(xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr) ;;
    esac
    if [ "${#portals[@]}" -gt 0 ] && ! have xdg-desktop-portal; then
        pm_install "${portals[@]}" || ui_warn "portals not installed"
    fi
    ui_ok "portals" "xdg-desktop-portal ready"

    # 2) Audio stack (volume OSD): wireplumber + pipewire.
    if ! have wpctl; then
        case "$FAMILY" in
            arch) pm_install pipewire pipewire-pulse wireplumber ;;
            debian) pm_install pipewire pipewire-pulse wireplumber ;;
            fedora) pm_install pipewire pipewire-pulseaudio wireplumber ;;
            suse) pm_install pipewire pipewire-pulseaudio wireplumber ;;
        esac || ui_warn "audio stack not installed"
    fi
    if [ "$DRY_RUN" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user enable --now pipewire.socket pipewire-pulse.socket wireplumber.service \
            >/dev/null 2>&1 || true
    fi
    ui_ok "audio" "pipewire/wireplumber enabled"

    # 3) Polkit agent: suspend/reboot from the UI go through logind.
    if ! pgrep -f "polkit.*authentication-agent" >/dev/null 2>&1; then
        case "$FAMILY" in
            arch) pm_install polkit-gnome ;;
            debian) pm_install polkit-gnome ;;
            fedora) pm_install polkit-gnome ;;
            suse) pm_install polkit-gnome ;;
        esac || true
        niri_spawn 'spawn-at-startup "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1"'
    fi
    ui_ok "polkit" "agent autostarted"

    # 4) System services that already exist: just enable them.
    if [ "$DRY_RUN" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
        if systemctl list-unit-files NetworkManager.service >/dev/null 2>&1; then
            as_root systemctl enable --now NetworkManager.service >/dev/null 2>&1 || true
        fi
        if [ -d /sys/class/bluetooth ] || have bluetoothctl; then
            as_root systemctl enable --now bluetooth.service >/dev/null 2>&1 || true
        fi
    fi
    ui_ok "services" "NetworkManager/Bluetooth enabled when present"

    # 5) Session autostart of the helper daemons.
    install_wallpaper_daemon
    if have awww; then
        niri_spawn 'spawn-at-startup "awww-daemon"'
    fi
}

install_runtime() {
    detect_family
    [ "$UPDATE" -eq 1 ] && detect_terminal
    if [ "$DRY_RUN" -eq 0 ] && [ -r /dev/tty ] && [ "$TERMINAL_ASKED" -eq 0 ] \
        && [ "$EUID" -ne 0 ] && [ "$UPDATE" -eq 0 ]; then
        ask_terminal
    fi
    [ -n "$TERMINAL" ] || TERMINAL="kitty"
    [ "$TERMINAL_ASKED" -eq 0 ] && TERMINAL="kitty"
    local -a pkgs=()
    local component pkg
    # niri: hard requirement
    if ! component_present niri; then
        pkgs+=("$(runtime_pkg niri)")
    else
        ui_ok "niri" "$(niri --version 2>/dev/null | head -1 || echo installed)"
    fi
    if ! component_present "$TERMINAL"; then
        pkg="$(runtime_pkg "$TERMINAL")"
        [ -n "$pkg" ] && pkgs+=("$pkg")
    else
        ui_ok "terminal" "$TERMINAL already installed"
    fi
    if [ "$DO_EXTRAS" -eq 1 ]; then
        for component in wl-clipboard brightnessctl; do
            component_present "$component" || pkgs+=("$(runtime_pkg "$component")")
        done
    fi
    if [ "${#pkgs[@]}" -eq 0 ]; then
        ui_ok "runtime" "nothing else to install"
        return 0
    fi
    local joined
    joined="$(printf '%s ' "${pkgs[@]}")"
    joined="${joined% }"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "runtime" "[dry-run] would install: $joined"
        return 0
    fi
    local pm; pm="$(package_manager)"
    [ -n "$pm" ] || { ui_warn "no package manager: install $joined"; return 0; }
    sudo_warmup
    case "$pm" in
        pacman) run_spin "runtime" as_root pacman -S --needed --noconfirm "${pkgs[@]}" ;;
        apt-get) run_spin "runtime" as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${pkgs[@]}" ;;
        dnf) run_spin "runtime" as_root dnf install -y "${pkgs[@]}" ;;
        zypper) run_spin "runtime" as_root zypper --non-interactive install "${pkgs[@]}" ;;
    esac || {
        ui_warn "could not install: $joined"
        ui_warn "niri comes from the distribution repository; on Debian it needs trixie or later"
        ui_warn "(see https://yalter.github.io/niri/Packaging.html)"
    }
    component_present niri || ui_warn "niri is still missing: install it before starting LARD"
    return 0
}

# ---------------------------------------------------------------------------
# Files

install_keep() { # src dst mode label
    local src="$1" dst="$2" mode="$3" label="$4"
    if [ -e "$dst" ] && [ "$FORCE" -eq 1 ] && [ "$REPAIR" -eq 1 ]; then
        case "$label" in
            config|"user config")
                ui_skip "$label" "kept during repair: $dst"
                return 0 ;;
        esac
    fi
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
# Boot: how the session comes up after a reboot.

BOOT_MODE="${LARD_BOOT:-}"

ask_boot() {
    if [ "$UPDATE" -eq 1 ] && [ -z "$BOOT_MODE" ]; then
        detect_boot_mode
        return 0
    fi
    [ -n "$BOOT_MODE" ] && return 0
    if [ "$DRY_RUN" -eq 1 ] || [ ! -r /dev/tty ]; then BOOT_MODE=greeter; return 0; fi
    printf '\n  %s?%s %s\n' "$C_GOLD" "$C_RESET" "How should the session start at boot?"
    printf '     %s1)%s greetd + LARD greeter (login screen) [default]\n' "$C_BOLD" "$C_RESET"
    printf '     %s2)%s niri on tty1 (niri-session, no display manager)\n' "$C_BOLD" "$C_RESET"
    printf '     %s3)%s leave the boot setup alone\n' "$C_BOLD" "$C_RESET"
    printf '  %s[1]%s ' "$C_GREY" "$C_RESET"
    local answer=""
    read -r answer < /dev/tty || answer=""
    case "${answer:-1}" in
        2) BOOT_MODE=tty ;;
        3) BOOT_MODE=none ;;
        *) BOOT_MODE=greeter ;;
    esac
    return 0
}

# Which boot arrangement is already installed? Used by --update so an update
# never changes the login setup behind the user's back.
detect_boot_mode() {
    if [ -f /etc/systemd/system/getty@tty1.service.d/autologin.conf ]; then
        BOOT_MODE=tty
    elif [ -f /etc/greetd/config.toml ] && grep -q '/etc/lard/greeter.kdl' /etc/greetd/config.toml 2>/dev/null; then
        BOOT_MODE=greeter
    else
        BOOT_MODE=none
    fi
}

# Keep the terminal the user picked the first time instead of resetting it to
# kitty on every update.
detect_terminal() {
    local binds="${XDG_CONFIG_HOME:-$HOME/.config}/niri/lard/binds.kdl"
    local term=""
    if [ -f "$binds" ]; then
        term="$(sed -n '/Mod+T /{s/.*spawn "\([^"]*\)".*/\1/p;q}' "$binds")"
    fi
    if [ -n "$term" ]; then
        TERMINAL="$term"
        TERMINAL_ASKED=1
    fi
}

# Remove the tty1 stanza written by older versions (and by tty mode).
remove_shell_autostart() {
    local profile="${1:-}"
    if [ -z "$profile" ]; then
        local user="${SUDO_USER:-${USER:-$(id -un)}}"
        [ "$user" = "root" ] && user="$(getent passwd 1000 | cut -d: -f1)"
        profile="$(getent passwd "$user" | cut -d: -f6)/.bash_profile"
    fi
    [ -f "$profile" ] || return 0
    grep -q '# >>> lard autologin' "$profile" 2>/dev/null || return 0
    cp -a "$profile" "$profile.bak-$(date +%Y%m%d%H%M%S)"
    sed -i '/# >>> lard autologin/,/# <<< lard autologin/d' "$profile"
    ui_ok "boot" "removed the old tty1 autologin stanza from $profile"
}

# No display manager: starting a login on tty1 launches niri (and LARD with it).
install_tty_autostart() {
    local user="${SUDO_USER:-${USER:-$(id -un)}}"
    if [ "$user" = "root" ]; then
        user="$(getent passwd 1000 | cut -d: -f1)"
    fi
    [ -n "$user" ] || user="$(id -un)"
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "boot" "[dry-run] would set autologin on tty1 for $user and run niri-session"
        return 0
    fi
    # 1) the shell that starts niri when it lands on tty1
    local profile
    profile="$(getent passwd "$user" | cut -d: -f6)/.bash_profile"
    remove_shell_autostart "$profile"
    cat >> "$profile" <<'EOP'

# >>> lard autologin
# Only on the getty autologin VT, and only when the parent is really agetty:
# niri-session runs a login shell of its own. Without that check this stanza
# re-execs niri-session forever (black screen, 100% CPU, no niri).
if [ -z "$WAYLAND_DISPLAY" ] && [ -z "$LARD_AUTOSTART" ] \
    && [ "$(tty)" = "/dev/tty1" ] \
    && [ "$(ps -o comm= -p "$PPID" 2>/dev/null)" = "agetty" ]; then
    export LARD_AUTOSTART=1
    exec niri-session
fi
# <<< lard autologin
EOP
    [ "$(id -u)" -eq 0 ] && as_root chown "$user" "$profile"
    ui_ok "boot" "niri-session on tty1 login ($profile)"
    # 2) tty1 logs the user in automatically: no login screen to configure
    local agetty=""
    local candidate
    for candidate in /usr/bin/agetty /sbin/agetty /bin/agetty; do
        [ -x "$candidate" ] && { agetty="$candidate"; break; }
    done
    if [ -z "$agetty" ]; then
        ui_warn "agetty not found: tty1 cannot be configured for autologin"
        return 0
    fi
    local drop=/etc/systemd/system/getty@tty1.service.d/autologin.conf
    if command -v systemctl >/dev/null 2>&1; then
        as_root mkdir -p /etc/systemd/system/getty@tty1.service.d
        printf '[Service]\nExecStart=\nExecStart=-%s --autologin %s --noclear %%I 38400 linux\n' \
            "$agetty" "$user" | as_root tee "$drop" >/dev/null
        as_root systemctl daemon-reload >/dev/null 2>&1 || true
        as_root systemctl enable getty@tty1.service >/dev/null 2>&1 || true
        # a display manager would steal the VT: one starter only
        if systemctl list-unit-files greetd.service >/dev/null 2>&1; then
            as_root systemctl disable greetd.service >/dev/null 2>&1 || true
        fi
        if as_root systemctl cat getty@tty1.service 2>/dev/null | grep -q -- "--autologin $user"; then
            ui_ok "boot" "autologin on tty1 for $user (no login screen needed)"
        else
            ui_warn "could not verify the tty1 autologin override: check $drop"
        fi
    fi
    return 0
}

install_boot() {
    [ "$DRY_RUN" -eq 1 ] && { ask_boot; ui_skip "boot" "[dry-run] mode: $BOOT_MODE"; return 0; }
    [ "$USER_MODE" -eq 1 ] && { ui_skip "boot" "not handled with --user"; return 0; }
    ask_boot
    case "$BOOT_MODE" in
        none) ui_skip "boot" "left as is"; return 0 ;;
        tty) install_tty_autostart; return 0 ;;
    esac

    # ---- greetd + LARD greeter ------------------------------------------------
    if ! command -v greetd >/dev/null 2>&1 && ! [ -f /usr/lib/systemd/system/greetd.service ] \
        && ! [ -f /usr/bin/greetd ] && ! [ -f /usr/sbin/greetd ]; then
        ui_step "boot" "installing greetd"
        sudo_warmup
        pm_install greetd || ui_warn "cannot install greetd"
    fi
    if ! command -v greetd >/dev/null 2>&1 && [ ! -f /usr/bin/greetd ] && [ ! -f /usr/sbin/greetd ]; then
        ui_warn "greetd is not available: falling back to autologin on tty1"
        install_tty_autostart
        return 0
    fi
    # The host compositor must exist *before* greetd starts: otherwise the
    # session command fails instantly and the service never becomes active.
    #
    # niri is used, not sway: LARD already requires niri, it implements
    # zwlr_layer_shell_v1, and this is the arrangement verified on the reference
    # machine. Sway is an extra dependency that is not packaged everywhere.
    if ! have niri; then
        ui_step "boot" "installing niri (host compositor for the greeter)"
        pm_install niri >>"$LOG_FILE" 2>&1 || true
    fi
    if ! have niri; then
        ui_warn "niri is missing: the greeter cannot start"
        ui_warn "falling back to autologin on tty1"
        install_tty_autostart
        return 0
    fi
    if ! getent passwd greeter >/dev/null 2>&1; then
        as_root useradd --system --create-home --shell /usr/sbin/nologin greeter >>"$LOG_FILE" 2>&1 \
            && ui_ok "boot" "created the 'greeter' user" \
            || ui_warn "cannot create the 'greeter' user"
    fi
    # The host compositor writes its state/logs in the home directory.
    local greeter_home
    greeter_home="$(getent passwd greeter | cut -d: -f6)"
    if [ -n "$greeter_home" ] && [ ! -d "$greeter_home" ]; then
        as_root mkdir -p "$greeter_home" && as_root chown greeter:greeter "$greeter_home"
    fi
    # wgpu needs the DRM render node, niri needs the seat devices: a greeter
    # outside those groups starts and dies with a permission error.
    local g
    for g in video render input; do
        getent group "$g" >/dev/null 2>&1 || continue
        if ! id -nG greeter 2>/dev/null | tr ' ' '\n' | grep -qx "$g"; then
            as_root usermod -aG "$g" greeter >>"$LOG_FILE" 2>&1 || true
            ui_ok "boot" "added 'greeter' to the $g group"
        fi
    done
    sudo_warmup
    # A stale tty1 autologin would fight greetd for the VT.
    if [ -f /etc/systemd/system/getty@tty1.service.d/autologin.conf ]; then
        as_root rm -f /etc/systemd/system/getty@tty1.service.d/autologin.conf
        ui_ok "boot" "removed the tty1 autologin override (greetd owns the VT)"
    fi
    remove_shell_autostart
    # Another display manager would keep the seat: greetd can start and then be
    # unable to draw because something else owns the VT.
    if command -v systemctl >/dev/null 2>&1; then
        local dm
        for dm in gdm gdm3 sddm lightdm lxdm ly cosmic-greeter; do
            if as_root systemctl is-enabled --quiet "$dm.service" >/dev/null 2>&1; then
                as_root systemctl disable "$dm.service" >>"$LOG_FILE" 2>&1 || true
                ui_ok "boot" "disabled $dm (greetd is the display manager now)"
            fi
        done
    fi
    # Configuration: greetd + the niri kiosk that hosts the greeter.
    local greetd_cfg=/etc/greetd/config.toml
    if [ -f "$greetd_cfg" ] && grep -q '/etc/lard/greeter.kdl' "$greetd_cfg" 2>/dev/null; then
        ui_ok "boot" "greetd already points at the LARD greeter"
    else
        if [ -f "$greetd_cfg" ]; then
            as_root cp -a "$greetd_cfg" "$greetd_cfg.bak-$(date +%Y%m%d%H%M%S)" || true
        fi
        as_root install -Dm644 "$SCRIPT_DIR/docs/greetd/config.toml" "$greetd_cfg" \
            || { ui_warn "cannot write $greetd_cfg"; install_tty_autostart; return 0; }
    fi
    # Absolute path: the greeter user's PATH does not include /usr/local/bin.
    if [ ! -f "$SCRIPT_DIR/docs/greeter-lard.kdl" ]; then
        ui_warn "docs/greeter-lard.kdl is missing: refusing to write an empty greeter config"
        install_tty_autostart
        return 0
    fi
    local tmp
    tmp="$(mktemp)"
    sed "s|/usr/local/bin/lard|$PREFIX/bin/lard|" "$SCRIPT_DIR/docs/greeter-lard.kdl" > "$tmp"
    as_root install -Dm644 "$tmp" /etc/lard/greeter.kdl
    rm -f "$tmp"
    # Sway kiosk, kept as the documented alternative for non-niri hosts.
    tmp="$(mktemp)"
    sed "s|\"lard greeter|\"$PREFIX/bin/lard greeter|" "$SCRIPT_DIR/docs/greetd/sway-greeter.conf" > "$tmp"
    as_root install -Dm644 "$tmp" /etc/lard/greeter-sway.conf
    rm -f "$tmp"
    if command -v systemctl >/dev/null 2>&1; then
        as_root systemctl daemon-reload >/dev/null 2>&1 || true
        as_root systemctl enable greetd.service >>"$LOG_FILE" 2>&1 || true
        # A machine whose default target is multi-user.target never starts the
        # display manager: it boots to a text login even with greetd enabled.
        as_root systemctl set-default graphical.target >>"$LOG_FILE" 2>&1 || true
        if [ "$UPDATE" -eq 1 ]; then
            ui_ok "boot" "greetd configuration refreshed (the new greeter is used at the next login)"
            return 0
        fi
        as_root systemctl restart greetd.service >>"$LOG_FILE" 2>&1 || true
        # "is-active" is not enough: greetd stays active while the greeter
        # session crash-loops. Success means a greeter process that lives for a
        # few seconds.
        local ok=0 i pid et
        for i in $(seq 1 20); do
            sleep 1
            if command -v pgrep >/dev/null 2>&1; then
                pid="$(pgrep -u greeter -f 'lard greeter' 2>/dev/null | head -n1)"
            else
                pid="$(ps -u greeter -o pid=,args= 2>/dev/null | awk '/lard greeter/ {print $1; exit}')"
            fi
            [ -n "$pid" ] || continue
            et="$(ps -o etimes= -p "$pid" 2>/dev/null | tr -d ' ')"
            if [ -n "$et" ] && [ "$et" -ge 3 ] 2>/dev/null; then ok=1; break; fi
        done
        if [ "$ok" -eq 1 ] && as_root systemctl is-active --quiet greetd.service; then
            ui_ok "boot" "greetd running: LARD greeter on VT1 (niri host)"
        ui_ok "boot" "default target $(systemctl get-default 2>/dev/null), greetd $(systemctl is-enabled greetd.service 2>/dev/null || echo '?')"
            return 0
        fi
        ui_warn "the greeter did not stay up: here is why"
        as_root journalctl -u greetd.service -b --no-pager 2>/dev/null | tail -30 >&2 || true
        ui_warn "falling back to autologin on tty1 (a working session beats a black screen)"
        install_tty_autostart
        return 0
    fi
    ui_warn "systemctl not available: greetd was configured but not enabled"
}

# ---------------------------------------------------------------------------
# Default wallpaper (CC0, shipped with the sources)

install_default_wallpaper() {
    local src="$SCRIPT_DIR/assets/wallpapers/lard-default.png"
    [ -f "$src" ] || return 0
    if [ "$DRY_RUN" -eq 1 ]; then
        ui_skip "wallpaper" "[dry-run] would install the CC0 default wallpaper"
        return 0
    fi
    local dst="$DESTDIR$PREFIX/share/lard/wallpapers/lard-default.png"
    as_root install -Dm644 "$src" "$dst"
    [ -n "$DESTDIR" ] && { ui_ok "wallpaper" "$dst"; return 0; }
    # Point the user configuration at it when no wallpaper was chosen yet.
    local cfg; cfg="$(user_config_path)"
    if [ -f "$cfg" ] && grep -Eq '^wallpaper *= *""' "$cfg"; then
        sed -i "s|^wallpaper *= *\"\"|wallpaper = \"$dst\"|" "$cfg"
    fi
    # Apply it now if a wallpaper daemon is already running.
    if command -v awww >/dev/null 2>&1; then
        awww img "$dst" --transition-type fade --transition-duration 0.6 >/dev/null 2>&1 || true
    fi
    ui_ok "wallpaper" "CC0 default wallpaper: $dst"
}

# ---------------------------------------------------------------------------
# Autostart

enable_autostart() {
    if [ "$DO_ENABLE" -eq 0 ] || staged || [ "$USER_MODE" -eq 1 ]; then
        ui_skip "autostart" "not requested"
        return 0
    fi
    # The user unit is the reliable path: niri-session starts
    # graphical-session.target, and the unit is ordered after it. It also
    # restarts the shell if it crashes, which a spawn-at-startup does not.
    if command -v systemctl >/dev/null 2>&1; then
        if [ "$DRY_RUN" -eq 1 ]; then
            ui_skip "autostart" "[dry-run] would enable lard.service"
            return 0
        fi
        systemctl --user daemon-reload >/dev/null 2>&1 || true
        if systemctl --user enable lard.service >/dev/null 2>&1; then
            ui_ok "autostart" "lard.service (user systemd, after the graphical session)"
            return 0
        fi
    fi
    # Fallback for systems without a usable user manager.
    local cfg
    cfg="$(niri_config_path)"
    if [ -f "$cfg" ]; then
        if grep -Eq '^[[:space:]]*spawn-at-startup[^/]*lard' "$cfg"; then
            ui_ok "autostart" "already configured in $cfg"
            return 0
        fi
        if [ "$DRY_RUN" -eq 1 ]; then
            ui_skip "autostart" "[dry-run] would add spawn-at-startup \"$PREFIX/bin/lard\""
            return 0
        fi
        cp -a "$cfg" "$cfg.bak-$(date +%Y%m%d%H%M%S)"
        printf '\n// >>> lard (autostart, managed by install.sh)\nspawn-at-startup "%s/bin/lard"\n// <<< lard\n' \
            "$PREFIX" >> "$cfg"
        ui_ok "autostart" "spawn-at-startup added to $cfg"
        return 0
    fi
    ui_warn "autostart not configured: add spawn-at-startup \"$PREFIX/bin/lard\" to your niri config"
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
    if [ "$UPDATE" -eq 1 ]; then
        ui_step "update" "keeping the configuration and the boot setup"
    elif [ "$REPAIR" -eq 0 ] && [ "$DRY_RUN" -eq 0 ] && [ "$USER_MODE" -eq 0 ] \
        && [ -x "$PREFIX/bin/lard" ] && [ -r /dev/tty ]; then
        printf '  %s!%s LARD is already installed (%s).\n\n' "$C_GOLD" "$C_RESET" "$PREFIX/bin/lard"
        printf '     %s1)%s Repair — fix everything, keep my configuration (default)\n' "$C_BOLD" "$C_RESET"
        printf '     %s2)%s Reinstall — replace the managed files too\n' "$C_BOLD" "$C_RESET"
        printf '     %s3)%s Cancel\n' "$C_BOLD" "$C_RESET"
        printf '  %s[1]%s ' "$C_GREY" "$C_RESET"
        local answer=""
        read -r answer < /dev/tty || answer=""
        case "${answer:-1}" in
            2) REPAIR=0; FORCE=1 ;;
            3) ui_skip "install" "cancelled"; exit 0 ;;
            *) REPAIR=1; FORCE=1 ;;
        esac
        if [ "$REPAIR" -eq 1 ]; then
            ui_step "repair" "checking and fixing the existing installation"
        fi
    fi
    printf '  %s%s%s\n' "$C_GREY" "prefix $PREFIX · config $SYSCONFDIR${DESTDIR:+ · staging $DESTDIR}" "$C_RESET"

    sudo_warmup
    install_deps
    install_fonts
    install_runtime
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
    Mod+Return  { spawn "${TERMINAL:-kitty}"; }
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

    install_default_wallpaper
    migrate_wallpaper_dir
    install_niri_config
    install_terminal_config
    enable_autostart
    install_boot
    setup_system

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
    if [ "$UPDATE" -eq 1 ]; then
        printf '\n  %s✓ LARD updated%s\n' "$C_GREEN$C_BOLD" "$C_RESET"
        if command -v systemctl >/dev/null 2>&1 \
            && systemctl --user is-active --quiet lard.service 2>/dev/null; then
            ( sleep 1; systemctl --user restart lard.service ) >/dev/null 2>&1 &
            printf '  %sthe bar restarts with the new binary%s\n\n' "$C_GREY" "$C_RESET"
        fi
        return 0
    fi
    printf '\n  %s✓ LARD installed%s\n' "$C_GREEN$C_BOLD" "$C_RESET"
    printf '  %sready:%s niri %s · fonts %s · portals %s · autostart %s\n' \
        "$C_GREY" "$C_RESET" \
        "$(command -v niri >/dev/null 2>&1 && echo yes || echo no)" \
        "$(fc-list 2>/dev/null | wc -l)" \
        "$(command -v xdg-desktop-portal >/dev/null 2>&1 && echo yes || echo no)" \
        "$([ -f "${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl" ] && grep -q 'spawn-at-startup.*lard' "${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl" && echo yes || echo no)"
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
