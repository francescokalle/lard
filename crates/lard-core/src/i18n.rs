//! UI language.
//!
//! The source strings are English and **are the keys**: `t("Control Center")`
//! returns the translation when the language is not English. This keeps the code
//! readable and makes translations cheap to add:
//!
//! * built-in languages live in [`IT`] below;
//! * anybody can drop a file in `~/.config/lard/lang/<code>.toml` with
//!   `"Control Center" = "Centro de control"` and it overrides the built-ins;
//! * a missing key falls back to English, so a partial translation never breaks
//!   the UI.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    It,
}

impl Lang {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::It => "it",
        }
    }

    /// Parse a configured language (`auto`, `en`, `it`, `it_IT`, …).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "en" | "eng" | "english" => Some(Self::En),
            "it" | "ita" | "italian" | "italiano" => Some(Self::It),
            _ if value.starts_with("it") => Some(Self::It),
            _ if value.starts_with("en") => Some(Self::En),
            _ => None,
        }
    }

    /// `auto`: follow `$LANG`/`$LC_ALL`, defaulting to English.
    #[must_use]
    pub fn from_environment() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(key)
                && let Some(lang) = Self::parse(&value)
            {
                return lang;
            }
        }
        Self::En
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static EXTERNAL: OnceLock<HashMap<String, &'static str>> = OnceLock::new();

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

#[must_use]
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::It,
        _ => Lang::En,
    }
}

/// Translate a source string. Unknown keys return the English text.
#[must_use]
pub fn t(key: &'static str) -> &'static str {
    if let Some(map) = EXTERNAL.get()
        && let Some(value) = map.get(key)
    {
        return value;
    }
    if lang() == Lang::En {
        return key;
    }
    IT.iter()
        .find(|(source, _)| *source == key)
        .map_or(key, |(_, translated)| *translated)
}

/// Load `~/.config/lard/lang/<code>.toml`, overriding the built-in table.
///
/// Format: one `"Source string" = "Translated string"` per line. Returns how
/// many keys were loaded.
pub fn load_external(code: &str) -> usize {
    let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .map(|base| base.join("lard").join("lang").join(format!("{code}.toml")))
    else {
        return 0;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut map: HashMap<String, &'static str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(key) = unquote(key.trim()) else {
            continue;
        };
        let Some(value) = unquote(value.trim()) else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        let leaked: &'static str = Box::leak(value.into_boxed_str());
        map.insert(key.to_string(), leaked);
    }
    let count = map.len();
    let _ = EXTERNAL.set(map);
    count
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(value.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Built-in Italian translation (source string → Italian).
const IT: &[(&str, &str)] = &[
    // Shell / settings
    ("Shell settings", "Impostazioni shell"),
    ("Appearance", "Aspetto"),
    ("Bar", "Barra"),
    ("Dock", "Dock"),
    ("Effects", "Effetti"),
    ("Screen", "Schermo"),
    ("Lock", "Blocco"),
    ("Displays", "Schermi"),
    ("Windows", "Finestre"),
    ("Advanced", "Avanzate"),
    ("Info", "Info"),
    ("Cancel", "Annulla"),
    ("Save", "Salva"),
    ("Close", "Chiudi"),
    ("Theme", "Tema"),
    ("System", "Sistema"),
    ("Light", "Chiaro"),
    ("Dark", "Scuro"),
    ("Palette", "Palette"),
    ("Dank gold", "Oro Dank"),
    ("Azure", "Azzurro"),
    ("Green", "Verde"),
    ("Pink", "Rosa"),
    ("Neutral", "Neutro"),
    ("Panel radius", "Angolo pannelli"),
    ("Card/tile radius", "Angolo card/tile"),
    ("Control radius", "Angolo controlli"),
    ("Chain on: every radius follows the card/tile value.", "Catena attiva: ogni angolo segue il valore card/tile."),
    ("Tip: the palette applies to bar, launcher, panels and lockscreen.", "Suggerimento: la palette vale per barra, launcher, pannelli e lockscreen."),
    ("Layout and blur apply on save", "Layout e blur si applicano al salvataggio"),
    ("Bar height", "Altezza barra"),
    ("Island radius", "Angolo isole"),
    ("Island opacity", "Opacità isole"),
    ("Workspace", "Workspace"),
    ("Show niri workspaces", "Mostra i workspace niri"),
    ("Window title", "Titolo finestra"),
    ("Title of the focused app", "Titolo dell'app focalizzata"),
    ("Clock", "Orologio"),
    ("Time and date in the centre", "Ora e data al centro"),
    ("CPU / RAM", "CPU / RAM"),
    ("Load in the control center", "Carico nel centro di controllo"),
    ("Volume", "Volume"),
    ("Volume slider in the control center", "Slider volume nel centro di controllo"),
    ("Network", "Rete"),
    ("Wi-Fi in the control center", "Wi-Fi nel centro di controllo"),
    ("Bluetooth", "Bluetooth"),
    ("Bluetooth in the control center", "Bluetooth nel centro di controllo"),
    ("Battery", "Batteria"),
    ("Battery status in the control center", "Stato batteria nel centro di controllo"),
    ("Dock enabled", "Dock attivo"),
    ("Application island at the bottom", "Isola applicazioni in basso"),
    ("Running apps", "App in esecuzione"),
    ("Add open apps to the pinned ones", "Aggiungi le app aperte alle preferite"),
    ("Dock height", "Altezza dock"),
    ("Dock opacity", "Opacità dock"),
    ("Pinned apps", "App fissate"),
    ("Panel blur", "Blur pannelli"),
    ("Island blur", "Blur isole"),
    ("Reduce motion", "Riduci movimento"),
    ("Disable springs at start-up", "Disattiva le molle all'avvio"),
    ("Panel opacity", "Opacità pannelli"),
    ("Glass level", "Livello vetro"),
    ("Top right", "Alto destra"),
    ("Top left", "Alto sinistra"),
    ("Bottom right", "Basso destra"),
    ("Bottom left", "Basso sinistra"),
    ("Notification position", "Posizione notifiche"),
    ("Where toasts appear", "Dove compaiono i toast"),
    ("Screensaver", "Salvaschermo"),
    ("Full-screen fluid effect while the session is idle", "Effetto fluido a schermo intero quando la sessione è inattiva"),
    ("After", "Dopo"),
    ("Scene", "Scenario"),
    ("Arrow keys ← → switch them on the fly", "Frecce ← → per cambiarli al volo"),
    ("Random", "Casuale"),
    ("Different scene and composition on every entrance", "Scenario e composizione diversi a ogni ingresso"),
    ("Script scenes", "Scene da script"),
    ("Adds the scenes written in ~/.config/lard/scenes to the arrows", "Aggiunge le scene scritte in ~/.config/lard/scenes alle frecce"),
    ("Glass", "Vetro"),
    ("Entrance", "Ingresso"),
    ("Speed", "Velocità"),
    ("Intensity", "Intensità"),
    ("Dust", "Polvere"),
    ("Time and date in the screensaver corner", "Ora e data nell'angolo del salvaschermo"),
    ("Gradient", "Gradiente"),
    ("Desktop wallpaper", "Sfondo del desktop"),
    ("Lock screen background", "Sfondo della schermata di blocco"),
    ("Veil", "Velo"),
    ("How much the wallpaper is darkened behind the login card.", "Quanto il wallpaper viene scurito dietro la card di accesso."),
    ("Version", "Versione"),
    ("Compositor", "Compositor"),
    ("Directory", "Cartella"),
    ("Configuration", "Configurazione"),
    ("The configuration is TOML and is saved with a .toml.bak backup.", "La configurazione è TOML e viene salvata con backup .toml.bak."),
    ("Monitors and scale", "Schermi e scala"),
    ("Window management", "Gestione finestre"),
    ("Advanced niri", "Niri avanzate"),
    ("Monitors", "Schermi"),
    ("Resolution", "Risoluzione"),
    ("Scale", "Scala"),
    ("Shortcuts", "Scorciatoie"),
    ("Reduce motion (animations snap)", "Riduci movimento (animazioni istantanee)"),
    // Control center / popovers
    ("Control Center", "Centro di controllo"),
    ("Controls", "Controlli"),
    ("Do not disturb", "Non disturbare"),
    ("Notifications", "Notifiche"),
    ("Brightness", "Luminosità"),
    ("Power", "Alimentazione"),
    ("Lock", "Blocco"),
    ("Session", "Sessione"),
    ("Log out", "Esci"),
    ("Suspend", "Sospendi"),
    ("Restart", "Riavvia"),
    ("Power off", "Spegni"),
    ("Lock the session", "Blocca la sessione"),
    ("Close the session", "Chiudi la sessione"),
    ("Suspend the system", "Metti in sospensione il sistema"),
    ("Restart the system", "Riavvia il sistema"),
    ("Power off the system", "Spegni il sistema"),
    ("Choose an action for the system", "Scegli un'azione per il sistema"),
    ("Actions run immediately.", "Le azioni vengono eseguite immediatamente."),
    ("Off", "Spento"),
    ("Active", "Attivo"),
    ("Absent", "Assente"),
    ("On", "Acceso"),
    ("Unavailable", "Non disponibile"),
    ("None", "Nessuna"),
    ("Muted", "Muto"),
    ("Connected", "Connesso"),
    ("Disconnected", "Disconnesso"),
    ("Connect", "Connetti"),
    ("Disconnect", "Disconnetti"),
    ("Scan", "Scansiona"),
    ("Scanning…", "Scansione…"),
    ("Searching…", "Ricerca…"),
    ("No devices", "Nessun dispositivo"),
    ("No networks found", "Nessuna rete trovata"),
    ("No network connected", "Nessuna rete connessa"),
    ("Wi-Fi off", "Wi-Fi spento"),
    ("Wi-Fi disabled", "Wi-Fi disattivato"),
    ("Turn Wi-Fi on", "Attiva Wi-Fi"),
    ("Turn Wi-Fi on to see available networks.", "Attiva il Wi-Fi per vedere le reti disponibili."),
    ("Turn Bluetooth on", "Attiva Bluetooth"),
    ("Turn Bluetooth on to look for devices.", "Attiva il Bluetooth per cercare i dispositivi."),
    ("Manage networks", "Gestisci reti"),
    ("Manage devices", "Gestisci dispositivi"),
    ("Wi-Fi login", "Accesso Wi-Fi"),
    ("Wi-Fi password", "Password Wi-Fi"),
    ("Username", "Nome utente"),
    ("Password", "Password"),
    ("Pair", "Associa"),
    ("Bluetooth unavailable", "Bluetooth non disponibile"),
    ("Bluetooth off", "Bluetooth spento"),
    ("Time to full", "Tempo alla carica"),
    ("Time left", "Tempo rimanente"),
    ("Power draw", "Potenza"),
    ("Charge", "Carica"),
    ("Health", "Salute"),
    ("Cycles", "Cicli"),
    ("Voltage", "Tensione"),
    ("External", "Esterna"),
    ("No battery", "Nessuna batteria"),
    ("System monitor", "Monitor di sistema"),
    ("Processes · CPU", "Processi · CPU"),
    ("Processes · memory", "Processi · memoria"),
    ("Uptime", "Attivo da"),
    ("Notifications", "Notifiche"),
    ("No notifications", "Nessuna notifica"),
    // Battery / power labels from the services
    ("Charging", "In carica"),
    ("Discharging", "In scarica"),
    ("Full", "Carica"),
    ("Paused (external power)", "In pausa (alimentazione esterna)"),
    ("External power", "Alimentazione esterna"),
    ("Unknown", "Sconosciuto"),
    // Launcher / OSD / screensaver
    ("Search applications…", "Cerca applicazioni…"),
    ("Searching files…", "Cerco file…"),
    ("No results", "Nessun risultato"),
    ("↑↓ navigate   wheel scrolls   ↵ open   esc close", "↑↓ naviga   rotella scorre   ↵ apri   esc chiudi"),
    ("audio muted", "audio disattivato"),
    ("← → scene · press any key to exit", "← → scenario · premi un tasto per uscire"),
    ("Notification", "Notifica"),
    // Lock screen / greeter
    ("Session locked", "Sessione bloccata"),
    ("Enter your password", "Inserisci la password"),
    ("Verifying…", "Verifica in corso…"),
    ("Invalid credentials", "Credenziali non valide"),
    ("Authentication failed", "Autenticazione non riuscita"),
    ("Authentication service unavailable", "Servizio di autenticazione non disponibile"),
    ("Password expired", "Password scaduta"),
    ("Account no longer valid", "Account non più valido"),
    // Screensaver effects
    ("Liquid", "Liquido"),
    ("Mercury", "Mercurio"),
    ("Waves", "Onde"),
    ("Grid", "Griglia"),
    ("Dust", "Polvere"),
    ("Vortex", "Vortice"),
    ("Storm", "Tempesta"),
];
