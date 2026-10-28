#!/usr/bin/env bash
#
# LARD installer
#
#   ./install.sh                 build + install + avvio automatico (sudo solo per /etc e /usr/local)
#   ./install.sh --install-deps  installa prima le dipendenze di build
#   ./install.sh --niri          aggiunge anche le bind a ~/.config/niri/config.kdl
#   ./install.sh --user          installa solo per l'utente corrente (~/.local)
#   ./install.sh --uninstall     rimuove binario/PAM/docs (la config resta)
#
set -Eeuo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_NAME="${0##*/}"

PREFIX="/usr/local"
SYSCONFDIR="/etc"
DESTDIR=""
USER_MODE=0
SKIP_BUILD=0
FORCE=0
INSTALL_DEPS=0
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
CARGO_TARGET_DIR_OVERRIDE="${CARGO_TARGET_DIR:-}"

log()  { printf '\033[1;34m::\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merrore:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
LARD installer

USO
  ./install.sh [opzioni]

OPZIONI PRINCIPALI
  --install-deps        installa le dipendenze di build con il package manager
  --niri                aggiunge le bind a ~/.config/niri/config.kdl (con backup)
  --user                installa solo per l'utente: ~/.local/bin e ~/.config
  --root DIR            staging per packaging (DESTDIR); non usa sudo
  --prefix DIR          prefisso di installazione (default /usr/local)
  --sysconfdir DIR      directory di configurazione (default /etc)
  --skip-build          usa un binario già compilato
  --binary PATH         usa un binario specifico (default target/release/...)
  --force               sovrascrive config/PAM esistenti dopo backup
  --uninstall           disinstalla
  --purge               con --uninstall rimuove anche la config
  --dry-run             mostra cosa farebbe, senza modificare nulla
  --no-pam              non installa /etc/pam.d/lard-greeter
  --no-config           non installa config.toml
  --no-greeter          non installa gli asset greeter
  --no-docs             non installa la documentazione
  --no-systemd          non installa la unit utente
  --no-enable           non abilitare l'avvio automatico di LARD
                        (default: abilita lard.service, o le bind niri se la
                        unit è già attiva)
  -y, --yes             non chiedere conferma per la disinstallazione
  -h, --help            questo aiuto

ESEMPI
  ./install.sh
  ./install.sh --install-deps
  ./install.sh --niri
  ./install.sh --user
  ./install.sh --root /tmp/pkg-stage --prefix /usr
  ./install.sh --uninstall
EOF
}

need_arg() {
    [ "$#" -ge 2 ] || die "l'opzione $1 richiede un valore"
}

normalize_dir() {
    local value="${1%/}"
    printf '%s' "${value:-/}"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --install-deps) INSTALL_DEPS=1 ;;
        --niri) DO_NIRI=1 ;;
        --user) USER_MODE=1 ;;
        --root)
            need_arg "$@"; DESTDIR="$(normalize_dir "$2")"; shift ;;
        --root=*) DESTDIR="$(normalize_dir "${1#*=}")" ;;
        --prefix)
            need_arg "$@"; PREFIX="$(normalize_dir "$2")"; shift ;;
        --prefix=*) PREFIX="$(normalize_dir "${1#*=}")" ;;
        --sysconfdir)
            need_arg "$@"; SYSCONFDIR="$(normalize_dir "$2")"; shift ;;
        --sysconfdir=*) SYSCONFDIR="$(normalize_dir "${1#*=}")" ;;
        --skip-build) SKIP_BUILD=1 ;;
        --binary)
            need_arg "$@"; BINARY_OVERRIDE="$2"; shift ;;
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
        *) die "opzione sconosciuta: $1 (usa --help)" ;;
    esac
    shift
done

if [ "$PURGE" -eq 1 ]; then
    UNINSTALL=1
fi
if [ "$USER_MODE" -eq 1 ] && [ -n "$DESTDIR" ]; then
    die "--user e --root non possono essere usati insieme"
fi
if [ "$USER_MODE" -eq 1 ]; then
    PREFIX="${HOME}/.local"
    DO_PAM=0
    DO_GREETER=0
    DO_SYSTEMD=0
fi
if [ -n "$DESTDIR" ]; then
    # Packaging: never touch the host, never ask for sudo.
    log "modalità staging: DESTDIR=$DESTDIR"
fi

if [ "$EUID" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$UNINSTALL" -eq 0 ] && [ -z "$DESTDIR" ]; then
    die "esegui ./install.sh come utente normale ($SUDO_USER), non con sudo. Lo script chiederà sudo solo per i file di sistema."
fi
if [ "$UNINSTALL" -eq 0 ] && [ -z "$DESTDIR" ] && [ "$USER_MODE" -eq 0 ] && [ "$EUID" -ne 0 ]; then
    command -v sudo >/dev/null 2>&1 || die "serve sudo per installare in $PREFIX e $SYSCONFDIR; usa --user per un'installazione locale"
fi

# ---------------------------------------------------------------------------
# Dipendenze

check_build_deps() {
    local missing=()
    local cmd pkg
    for cmd in cargo rustc pkg-config cc install; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    if command -v pkg-config >/dev/null 2>&1; then
        for pkg in wayland-client pam fontconfig xkbcommon; do
            pkg-config --exists "$pkg" || missing+=("pkg:$pkg")
        done
    fi
    if [ "${#missing[@]}" -eq 0 ]; then
        return 0
    fi
    warn "dipendenze mancanti: ${missing[*]}"
    cat >&2 <<'EOF'

Installa i prerequisiti, ad esempio:

  Debian/Ubuntu:
    sudo apt install build-essential pkg-config libwayland-dev libpam0g-dev \
                     libfontconfig1-dev libxkbcommon-dev libvulkan-dev

  Arch:
    sudo pacman -S --needed base-devel pkgconf wayland pam fontconfig \
                         libxkbcommon vulkan-icd-loader

  Fedora:
    sudo dnf install gcc gcc-c++ make pkgconf-pkg-config wayland-devel \
                     pam-devel fontconfig-devel libxkbcommon-devel \
                     vulkan-loader-devel

Oppure rilancia:  ./install.sh --install-deps
EOF
    return 1
}

install_deps() {
    log "installazione dipendenze di build"
    if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
        command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 \
            || die "serve curl o wget per installare Rust via rustup"
        log "Rust non trovato: installo la toolchain stabile con rustup"
        if command -v curl >/dev/null 2>&1; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
                | sh -s -- -y --profile minimal
        else
            wget -qO- https://sh.rustup.rs | sh -s -- -y --profile minimal
        fi
        # shellcheck disable=SC1091
        [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if command -v apt-get >/dev/null 2>&1; then
        as_root apt-get update
        as_root apt-get install -y build-essential pkg-config libwayland-dev \
            libpam0g-dev libfontconfig1-dev libxkbcommon-dev libvulkan-dev
    elif command -v pacman >/dev/null 2>&1; then
        as_root pacman -S --needed --noconfirm base-devel pkgconf wayland pam \
            fontconfig libxkbcommon vulkan-icd-loader
    elif command -v dnf >/dev/null 2>&1; then
        as_root dnf install -y gcc gcc-c++ make pkgconf-pkg-config wayland-devel \
            pam-devel fontconfig-devel libxkbcommon-devel vulkan-loader-devel
    elif command -v zypper >/dev/null 2>&1; then
        as_root zypper --non-interactive install gcc gcc-c++ make pkg-config \
            wayland-devel pam-devel fontconfig-devel libxkbcommon-devel \
            vulkan-loader-devel
    else
        die "package manager non riconosciuto; installa manualmente i prerequisiti"
    fi
    check_build_deps || die "dipendenze ancora mancanti dopo l'installazione"
}

# ---------------------------------------------------------------------------
# Comandi come root / staging

as_root() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run]'
        printf ' %q' "$@"
        printf '\n'
        return 0
    fi
    if [ "$EUID" -eq 0 ]; then
        "$@"
    else
        sudo "$@"
    fi
}

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run]'
        printf ' %q' "$@"
        printf '\n'
        return 0
    fi
    "$@"
}

is_staged() {
    [ -n "$DESTDIR" ]
}

install_system_file() {
    local src="$1" dst="$2" mode="${3:-644}"
    if is_staged || [ "$USER_MODE" -eq 1 ]; then
        run install -Dm"$mode" "$src" "$dst"
    else
        as_root install -Dm"$mode" "$src" "$dst"
    fi
}

copy_system_tree() {
    local src="$1" dst="$2"
    if is_staged || [ "$USER_MODE" -eq 1 ]; then
        run install -d "$dst"
        run cp -a "$src/." "$dst/"
    else
        as_root install -d "$dst"
        as_root cp -a "$src/." "$dst/"
    fi
}

backup_path() {
    local path="$1"
    [ -e "$path" ] || return 0
    local backup="${path}.bak.$(date +%Y%m%d-%H%M%S)"
    if is_staged || [ "$USER_MODE" -eq 1 ]; then
        run cp -a "$path" "$backup"
    else
        as_root cp -a "$path" "$backup"
    fi
    warn "backup: $backup"
}

install_keep() {
    local src="$1" dst="$2" mode="${3:-644}" label="${4:-file}"
    if [ -e "$dst" ]; then
        if [ "$FORCE" -eq 1 ]; then
            backup_path "$dst"
            install_system_file "$src" "$dst" "$mode"
            ok "$label aggiornato: $dst"
        else
            log "$label già presente, mantenuto: $dst (usa --force per sostituirlo)"
        fi
    else
        install_system_file "$src" "$dst" "$mode"
        ok "$label installato: $dst"
    fi
}

# ---------------------------------------------------------------------------
# Build

build_binary() {
    if [ -n "$BINARY_OVERRIDE" ]; then
        [ -x "$BINARY_OVERRIDE" ] || die "binario non eseguibile: $BINARY_OVERRIDE"
        BIN="$BINARY_OVERRIDE"
        return
    fi

    local target_dir="${CARGO_TARGET_DIR_OVERRIDE:-$SCRIPT_DIR/target}"
    case "$target_dir" in
        /*) ;;
        *) target_dir="$SCRIPT_DIR/$target_dir" ;;
    esac
    BIN="$target_dir/release/lard"

    if [ "$SKIP_BUILD" -eq 1 ]; then
        [ -x "$BIN" ] || die "--skip-build richiesto ma $BIN non esiste"
        return
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] cargo build --release --locked --bin lard"
        return
    fi
    log "build release (--locked)"
    ( cd "$SCRIPT_DIR" && cargo build --release --locked --bin lard )
    [ -x "$BIN" ] || die "build completata ma $BIN non esiste"
}

# ---------------------------------------------------------------------------
# Config dell'utente

user_config_path() {
    if [ -n "${XDG_CONFIG_HOME:-}" ]; then
        printf '%s/lard/config.toml' "$XDG_CONFIG_HOME"
    else
        printf '%s/.config/lard/config.toml' "$HOME"
    fi
}

install_user_config() {
    [ "$DO_CONFIG" -eq 1 ] || return 0
    [ -z "$DESTDIR" ] || return 0
    if [ "$EUID" -eq 0 ]; then
        # In installazioni dirette come root non indoviniamo la home di un
        # utente: la config di sistema funge da fallback.
        return 0
    fi
    local dst
    dst="$(user_config_path)"
    if [ -e "$dst" ]; then
        if [ "$FORCE" -eq 1 ]; then
            backup_path "$dst"
            run install -Dm644 "$SCRIPT_DIR/config/config.toml" "$dst"
            ok "config utente aggiornata: $dst"
        else
            log "config utente già presente, mantenuta: $dst"
        fi
    else
        run install -Dm644 "$SCRIPT_DIR/config/config.toml" "$dst"
        ok "config utente installata: $dst"
    fi
}

# ---------------------------------------------------------------------------
# Avvio automatico

# Make LARD start with the session, without ever starting it twice: one
# mechanism only, and only if none is already configured.
enable_autostart() {
    [ "$DO_ENABLE" -eq 1 ] || { log "avvio automatico non richiesto (--no-enable)"; return 0; }
    [ -z "$DESTDIR" ] || return 0
    if [ "$USER_MODE" -eq 1 ]; then
        log "--user: avvio automatico non gestito (usa spawn-at-startup in niri)"
        return 0
    fi
    local niri_cfg
    niri_cfg="${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl"
    if command -v systemctl >/dev/null 2>&1 \
        && [ "$(systemctl --user is-enabled lard.service 2>/dev/null || true)" = "enabled" ]; then
        ok "avvio automatico già attivo: lard.service (systemd utente)"
        return 0
    fi
    if [ -f "$niri_cfg" ] && grep -Eq '^[[:space:]]*spawn-at-startup[^/]*lard' "$niri_cfg"; then
        ok "avvio automatico già configurato in $niri_cfg"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] abiliterei lard.service (systemd utente)"
        return 0
    fi
    if command -v systemctl >/dev/null 2>&1 \
        && systemctl --user daemon-reload >/dev/null 2>&1 \
        && systemctl --user enable --now lard.service >/dev/null 2>&1; then
        ok "avvio automatico attivo: lard.service (systemd utente)"
        return 0
    fi
    # No user systemd (or a session that does not know graphical-session):
    # niri's own spawn-at-startup is the portable path.
    if [ -f "$niri_cfg" ]; then
        if grep -Eq '^[[:space:]]*spawn-at-startup[^/]*lard' "$niri_cfg"; then
            ok "avvio automatico già configurato in $niri_cfg"
            return 0
        fi
        backup_path "$niri_cfg"
        printf '\n// >>> lard (avvio automatico, gestito da install.sh)\nspawn-at-startup "%s/bin/lard"\n// <<< lard\n' \
            "$PREFIX" >> "$niri_cfg"
        ok "avvio automatico aggiunto a $niri_cfg (spawn-at-startup)"
        return 0
    fi
    warn "avvio automatico non impostato: abilita lard.service o aggiungi"
    warn "  spawn-at-startup \"$PREFIX/bin/lard\"  alla configurazione di niri"
}

# ---------------------------------------------------------------------------
# Integrazione niri (opzionale e conservativa)

install_niri_binds() {
    [ "$DO_NIRI" -eq 1 ] || return 0
    [ -z "$DESTDIR" ] || return 0
    if [ "$EUID" -eq 0 ]; then
        warn "--niri ignorato quando eseguito come root"
        return 0
    fi
    local cfg
    if [ -n "${XDG_CONFIG_HOME:-}" ]; then
        cfg="$XDG_CONFIG_HOME/niri/config.kdl"
    else
        cfg="$HOME/.config/niri/config.kdl"
    fi
    if [ -f "$cfg" ] && grep -q "lard" "$cfg"; then
        log "LARD è già presente in $cfg; nessuna modifica"
        return 0
    fi
    if [ -f "$cfg" ] && grep -Eq 'Mod\+D|Mod\+Escape|Mod\+Shift\+E|Mod\+Shift\+Q' "$cfg"; then
        warn "$cfg contiene già Mod+D o Mod+Escape; non aggiungo bind duplicate."
        warn "Aggiungi manualmente (vedi docs/niri/config.kdl)."
        return 0
    fi
    local launch="$PREFIX/bin/lard"
    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] aggiungerei le bind LARD a $cfg (comando: $launch)"
        return 0
    fi
    if [ -f "$cfg" ]; then
        backup_path "$cfg"
    else
        run install -d "$(dirname "$cfg")"
        printf '%s\n' '// lard: file creato da install.sh' > "$cfg"
    fi
    cat >> "$cfg" <<EOF

// >>> lard (gestito da install.sh)
spawn-at-startup "$launch"

binds {
    Mod+D       { spawn "$launch" "ipc" "launcher"; }
    Mod+Shift+E { spawn "$launch" "ipc" "control-center"; }
    Mod+Escape  { spawn "$launch" "lock"; }
    Mod+Shift+Q { spawn "$launch" "ipc" "quit"; }
}
// <<< lard
EOF
    ok "bind niri aggiunte a $cfg (comando: $launch)"
}

# ---------------------------------------------------------------------------
# Installazione

do_install() {
    if [ "$SKIP_BUILD" -eq 0 ]; then
        check_build_deps || {
            if [ "$INSTALL_DEPS" -eq 1 ]; then
                install_deps
            else
                die "dipendenze di build mancanti (usa --install-deps)"
            fi
        }
    fi

    build_binary

    if [ "$EUID" -ne 0 ] && [ -z "$DESTDIR" ] && [ "$USER_MODE" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
        log "sudo verrà richiesto una volta per i file di sistema"
        sudo -v
    fi

    local bin_dst
    if [ "$USER_MODE" -eq 1 ]; then
        bin_dst="$HOME/.local/bin/lard"
    else
        bin_dst="$DESTDIR$PREFIX/bin/lard"
    fi
    install_system_file "$BIN" "$bin_dst" 755
    ok "binario: $bin_dst"

    if [ "$DO_CONFIG" -eq 1 ] && [ "$USER_MODE" -eq 0 ]; then
        install_keep "$SCRIPT_DIR/config/config.toml" \
            "${DESTDIR}${SYSCONFDIR}/lard/config.toml" 644 \
            "config di sistema"
    fi
    install_user_config

    if [ "$DO_PAM" -eq 1 ]; then
        install_keep "$SCRIPT_DIR/docs/pam.d/lard-greeter" \
            "${DESTDIR}${SYSCONFDIR}/pam.d/lard-greeter" 644 \
            "servizio PAM"
    fi

    if [ "$DO_GREETER" -eq 1 ]; then
        install_keep "$SCRIPT_DIR/docs/greetd/sway-greeter.conf" \
            "${DESTDIR}${SYSCONFDIR}/lard/greeter-sway.conf" 644 \
            "config greeter sway"
        install_keep "$SCRIPT_DIR/docs/greeter-niri.kdl" \
            "${DESTDIR}${SYSCONFDIR}/lard/greeter.kdl" 644 \
            "config greeter niri"
    fi

    if [ "$DO_SYSTEMD" -eq 1 ]; then
        local unit_tmp
        unit_tmp="$(mktemp)"
        sed "s|/usr/local/bin/lard|$PREFIX/bin/lard|" \
            "$SCRIPT_DIR/docs/systemd/lard.service" > "$unit_tmp"
        install_keep "$unit_tmp" \
            "$DESTDIR$PREFIX/lib/systemd/user/lard.service" 644 \
            "unit systemd utente"
        rm -f "$unit_tmp"
    fi

    if [ "$DO_DOCS" -eq 1 ]; then
        local doc_dst
        if [ "$USER_MODE" -eq 1 ]; then
            doc_dst="$HOME/.local/share/doc/lard"
        else
            doc_dst="$DESTDIR$PREFIX/share/doc/lard"
        fi
        copy_system_tree "$SCRIPT_DIR/docs" "$doc_dst"
        if [ -d "$SCRIPT_DIR/preview" ]; then
            copy_system_tree "$SCRIPT_DIR/preview" "$doc_dst/preview"
        fi
        install_system_file "$SCRIPT_DIR/README.md" "$doc_dst/README.md" 644
        install_system_file "$SCRIPT_DIR/LICENSE" "$doc_dst/LICENSE" 644
        ok "documentazione: $doc_dst"
    fi

    install_niri_binds
    enable_autostart

    if [ "$DRY_RUN" -eq 0 ] && [ -z "$DESTDIR" ] && [ -x "$bin_dst" ]; then
        "$bin_dst" version >/dev/null 2>&1 && ok "verifica binario: $("$bin_dst" version)" \
            || warn "il binario è installato ma non parte in questo ambiente (manca Wayland?)"
    fi

    echo
    ok "installazione completata"
    printf '  binario : %s\n' "$bin_dst"
    if [ "$USER_MODE" -eq 1 ]; then
        if [ "$DO_CONFIG" -eq 1 ]; then
            printf '  config  : %s\n' "$(user_config_path)"
        else
            echo "  config  : non installata (--no-config)"
        fi
        echo "  PAM     : non installato (--user); per il lockscreen serve un'installazione di sistema"
        case ":$PATH:" in
            *":$HOME/.local/bin:"*) ;;
            *) echo "  PATH    : aggiungi $HOME/.local/bin al PATH" ;;
        esac
    else
        if [ "$DO_CONFIG" -eq 1 ]; then
            printf '  config  : %s\n' "${DESTDIR}${SYSCONFDIR}/lard/config.toml"
        else
            echo "  config  : non installata (--no-config)"
        fi
        if [ "$DO_PAM" -eq 1 ]; then
            printf '  PAM     : %s\n' "${DESTDIR}${SYSCONFDIR}/pam.d/lard-greeter"
        fi
    fi
    echo
    echo "Prossimi passi:"
    if [ "$DO_ENABLE" -eq 1 ] && [ "$USER_MODE" -eq 0 ]; then
        echo "  1) LARD parte con la sessione (avvio automatico configurato)."
        echo "  2) avviala subito:  $PREFIX/bin/lard"
        echo "     oppure:          systemctl --user restart lard.service"
    else
        echo "  1) niri: aggiungi  spawn-at-startup \"lard\"  e le bind di docs/niri/config.kdl"
        echo "     (oppure rilancia ./install.sh --niri per aggiungerle automaticamente)."
        echo "  2) avvia la shell:  lard"
    fi
    if [ "$USER_MODE" -eq 1 ]; then
        echo "  3) lockscreen: richiede installazione di sistema (PAM in /etc/pam.d)"
    else
        echo "  3) lockscreen:      lard lock   (richiede il servizio PAM installato)"
        echo "  4) greeter greetd:  vedi ${DESTDIR}${SYSCONFDIR}/lard/ e docs/greetd/config.toml"
        if ! getent passwd greeter >/dev/null 2>&1; then
            echo "     nota: l'utente 'greeter' non esiste; crealo prima di usare greetd."
        fi
    fi
}

# ---------------------------------------------------------------------------
# Disinstallazione

do_uninstall() {
    local bin_dst etc_dir pam_file unit_file doc_dst
    if [ "$USER_MODE" -eq 1 ]; then
        bin_dst="$HOME/.local/bin/lard"
        doc_dst="$HOME/.local/share/doc/lard"
        etc_dir=""
        pam_file=""
        unit_file=""
    else
        bin_dst="$DESTDIR$PREFIX/bin/lard"
        doc_dst="$DESTDIR$PREFIX/share/doc/lard"
        etc_dir="$DESTDIR$SYSCONFDIR/lard"
        pam_file="$DESTDIR$SYSCONFDIR/pam.d/lard-greeter"
        unit_file="$DESTDIR$PREFIX/lib/systemd/user/lard.service"
    fi

    if [ "$ASSUME_YES" -ne 1 ] && [ "$DRY_RUN" -eq 0 ]; then
        if [ ! -t 0 ]; then
            die "disinstallazione non interattiva: aggiungi --yes"
        fi
        local answer
        read -r -p "Rimuovere LARD da questo sistema? [y/N] " answer
        case "$answer" in
            y|Y|yes|YES) ;;
            *) die "annullato" ;;
        esac
    fi

    remove_path() {
        local path="$1"
        [ -n "$path" ] || return 0
        [ -e "$path" ] || return 0
        if is_staged || [ "$USER_MODE" -eq 1 ]; then
            run rm -rf -- "$path"
        else
            as_root rm -rf -- "$path"
        fi
    }

    if [ -n "$unit_file" ] && [ "$DRY_RUN" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now lard.service >/dev/null 2>&1 || true
    fi
    remove_path "$bin_dst"
    remove_path "$doc_dst"
    remove_path "$pam_file"
    remove_path "$unit_file"
    if [ -n "$etc_dir" ]; then
        remove_path "$etc_dir/greeter-sway.conf"
        remove_path "$etc_dir/greeter.kdl"
    fi

    if [ "$PURGE" -eq 1 ]; then
        if [ -n "$etc_dir" ]; then
            remove_path "$etc_dir"
        fi
        if [ "$EUID" -ne 0 ] || [ "$USER_MODE" -eq 1 ]; then
            remove_path "$(user_config_path)"
        fi
    else
        echo
        if [ -z "$DESTDIR" ] && [ "$EUID" -ne 0 ]; then
            log "config utente preservata: $(user_config_path 2>/dev/null || true)"
            local niri_cfg="${XDG_CONFIG_HOME:-$HOME/.config}/niri/config.kdl"
            if [ -f "$niri_cfg" ] && grep -q '>>> lard' "$niri_cfg"; then
                log "blocco lard lasciato in $niri_cfg (rimuovilo manualmente)"
            fi
        fi
        [ -n "$etc_dir" ] && log "config di sistema preservata: $etc_dir/config.toml"
    fi

    if [ "$DRY_RUN" -eq 0 ]; then
        ok "LARD disinstallato"
    fi
}

if [ "$UNINSTALL" -eq 1 ]; then
    do_uninstall
else
    do_install
fi
