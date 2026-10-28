#!/usr/bin/env bash
# LARD — installazione con un comando.
#
#   curl -fsSL https://lard.francescocallegaro.com/install.sh | bash
#   curl -fsSL https://lard.francescocallegaro.com/install.sh | bash -s -- --install-deps
#
# Scarica il repository in una directory temporanea ed esegue il vero
# install.sh (che compila e installa). Tutti gli argomenti vengono passati
# all'installer: --install-deps, --niri, --user, --no-enable, --dry-run, ...
#
# Variabili utili:
#   LARD_REPO   repository da usare (default: github.com/francescokalle/lard)
#   LARD_REF    branch/tag da installare (default: main)
#   LARD_DIR    usa una copia già presente invece di scaricarla

set -Eeuo pipefail

REPO="${LARD_REPO:-https://github.com/francescokalle/lard}"
REF="${LARD_REF:-main}"
TMP=""
cleanup() { [ -n "$TMP" ] && rm -rf "$TMP"; }
trap cleanup EXIT

log() { printf '\033[1;34m::\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merrore:\033[0m %s\n' "$*" >&2; exit 1; }

if [ -n "${LARD_DIR:-}" ]; then
    src="$LARD_DIR"
    [ -x "$src/install.sh" ] || die "LARD_DIR=$src non contiene install.sh"
    log "uso la copia locale: $src"
else
    command -v git >/dev/null 2>&1 || die "serve git (installalo e riprova)"
    TMP="$(mktemp -d)"
    log "scarico LARD ($REF) da $REPO"
    git clone --depth 1 --branch "$REF" "$REPO" "$TMP/lard" >/dev/null 2>&1 \
        || die "clone fallito: controlla rete e nome del branch ($REF)"
    src="$TMP/lard"
fi

cd "$src"
log "avvio l'installer (compila in release; chiederà sudo per i file di sistema)"
./install.sh "$@"
