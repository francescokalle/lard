#!/usr/bin/env bash
# LARD — one-command installer.
#
#   curl -fsSL https://lard.francescocallegaro.com/install.sh | bash
#   curl -fsSL https://lard.francescocallegaro.com/install.sh | bash -s -- --niri
#
# Downloads a snapshot of the repository into a temporary directory and runs the
# real install.sh (which installs the build dependencies, compiles and installs).
# Every argument is passed through: --niri, --user, --no-deps, --skip-build,
# --dry-run, --uninstall, ...
#
# Environment:
#   LARD_REPO   repository (default https://github.com/francescokalle/lard)
#   LARD_REF    branch/tag to install (default main)
#   LARD_DIR    use an existing checkout instead of downloading
#
set -Eeuo pipefail

REPO="${LARD_REPO:-https://github.com/francescokalle/lard}"
REF="${LARD_REF:-main}"
TMP=""
cleanup() { [ -n "$TMP" ] && rm -rf "$TMP"; }
trap cleanup EXIT

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_RESET=$'\033[0m'; C_GOLD=$'\033[38;5;179m'; C_GREY=$'\033[90m'
    C_BOLD=$'\033[1m'; C_RED=$'\033[31m'; C_GREEN=$'\033[32m'
else
    C_RESET=""; C_GOLD=""; C_GREY=""; C_BOLD=""; C_RED=""; C_GREEN=""
fi
step() { printf '  %s◐%s %s%-10s%s %s%s%s\n' "$C_GOLD" "$C_RESET" "$C_BOLD" "$1" "$C_RESET" "$C_GREY" "${2:-}" "$C_RESET"; }
ok()   { printf '  %s✓%s %s%-10s%s %s%s%s\n' "$C_GREEN" "$C_RESET" "$C_BOLD" "$1" "$C_RESET" "$C_GREY" "${2:-}" "$C_RESET"; }
die()  { printf '  %s✗%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; exit 1; }

printf '\n%s' "$C_GOLD$C_BOLD"
printf '%s\n' "  ╭──────────────────────────────────────────────╮"
printf '%s\n' "  │  LARD — light as rust dust                   │"
printf '%s\n' "  │  native Wayland shell for niri               │"
printf '%s\n' "  ╰──────────────────────────────────────────────╯"
printf '%s\n\n' "$C_RESET"

if [ -n "${LARD_DIR:-}" ]; then
    src="$LARD_DIR"
    [ -x "$src/install.sh" ] || die "LARD_DIR=$src does not contain install.sh"
    ok "sources" "using $src"
else
    TMP="$(mktemp -d)"
    src="$TMP/lard"
    # A source tarball needs neither git nor credentials: it is the most robust
    # path and also the fastest one.
    tarball="https://codeload.github.com/${REPO#https://github.com/}/tar.gz/refs/heads/$REF"
    step "download" "$REF snapshot"
    if command -v curl >/dev/null 2>&1 && curl -fsSL --retry 2 --connect-timeout 15 "$tarball" 2>/dev/null | tar -xz -C "$TMP" 2>/dev/null; then
        mv "$TMP"/lard-* "$src" 2>/dev/null || true
    elif command -v wget >/dev/null 2>&1 && wget -qO- --timeout=20 "$tarball" 2>/dev/null | tar -xz -C "$TMP" 2>/dev/null; then
        mv "$TMP"/lard-* "$src" 2>/dev/null || true
    fi
    if [ ! -x "$src/install.sh" ] && command -v git >/dev/null 2>&1; then
        step "download" "falling back to git clone"
        rm -rf "$src"
        if ! GIT_TERMINAL_PROMPT=0 git clone --depth 1 --branch "$REF" "$REPO" "$src" 2>"$TMP/git.log"; then
            GIT_TERMINAL_PROMPT=0 git clone --depth 1 "$REPO" "$src" 2>>"$TMP/git.log" \
                || die "cannot download LARD: $(tail -n 2 "$TMP/git.log" | tr '\n' ' ') — check the network or LARD_REPO"
        fi
    fi
    [ -x "$src/install.sh" ] || die "download failed: no network to github.com or wrong LARD_REF ($REF)"
    ok "download" "$REF"
fi

cd "$src"
./install.sh "$@"
