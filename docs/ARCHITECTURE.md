# Architettura

Questo documento spiega *perché* il codice è diviso così e quali invarianti
tengono insieme il loop a 120 Hz. Il codice è la fonte di verità; qui ci sono le
decisioni e i punti delicati.

## 1. Obiettivi e non-obiettivi

Obiettivi:

- un solo linguaggio (Rust) e un solo processo per la shell;
- rendering hardware-accelerated diretto, senza toolkit;
- idle a costo ~zero, animazioni indipendenti dal refresh;
- lock/greeter separati e non privilegiati;
- configurazione senza ricompilare.

Non-obiettivi della 0.1:

- reimplementare un compositore Wayland (il greeter usa un compositore ospite);
- essere un clone di DankLinux/Noctalia a livello di plugin ecosystem;
- supportare X11.

## 2. Crate e direzione delle dipendenze

```
                    ┌──────────────┐
                    │  lard-core   │  config, colori, geometria, spring, desktop
                    └──────┬───────┘
             ┌─────────────┼─────────────┐
             ▼             ▼             ▼
      ┌────────────┐ ┌───────────────┐ ┌──────────────┐
      │ lard-render│ │ lard-services │ │  lard-greeter │
      └─────┬──────┘ └───────┬───────┘ └──────┬───────┘
            └────────┬───────┴────────────────┘
                     ▼
              ┌────────────┐
              │ lard-shell │
              └─────┬──────┘
                    ▼
              ┌─────────────┐
              │ lard-wayland│  (client Wayland + loop + input + IPC)
              └─────┬───────┘
                    ▼
              ┌───────────┐
              │   lard    │  binario/CLI
              └───────────┘
```

- `lard-core` non conosce Wayland, GPU o thread.
- `lard-render` non conosce niri né widget.
- `lard-services` non conosce pixel.
- `lard-wayland` è l'unico crate che tocca `wl_surface` e `wgpu::Surface`.
- `lard-shell`/`lard-greeter` producono solo un `Scene` (draw list).

## 3. Ciclo degli eventi

`calloop` possiede il loop. Sorgenti registrate:

1. `WaylandSource` — eventi del compositore, configure, input, frame callback.
2. `calloop::channel` dei servizi — stato niri, volume, batteria, rete, BT, notifiche.
3. `calloop::channel` dell'autenticazione — risultato PAM/greetd dal worker.
4. `calloop::channel` dell'IPC — comandi da `lard ipc`.
5. `Timer` dell'orologio — allineato al prossimo minuto (o secondo se il formato ha `%S`).
6. `Timer` della prossima scadenza OSD/toast — un solo timer, riarmato al bisogno.
7. Socket Unix in un thread dedicato (`$XDG_RUNTIME_DIR/lard.sock`).

Il loop chiama `dispatch(None)` e **si blocca** quando non c'è nulla da fare.

### 3.1 Frame scheduling

Stato per superficie: `needs_redraw`, `configured`, `frame_pending`.
Stato globale: `animating`, `force_update`, `callback_fired`.

Dopo ogni dispatch:

```
tick = callback_fired || force_update
if tick:
    dt = frame_clock.tick()            # clamp 0..100 ms
    animating = shell.update(dt) | greeter.update(dt)
drain_actions()
sync_surfaces()                        # crea/distrugge overlay
if shell.dirty || animating: mark_all_dirty()
arm_next_deadline()

for each surface with needs_redraw && configured && !frame_pending:
    wl_surface.frame(...)              # callback PRIMA del commit
    draw(); needs_redraw = false

if animating: mark_all_dirty()         # il prossimo callback ridisegna
```

Perché `frame()` prima del present: il callback Wayland scatta al commit
successivo. `wgpu::Queue::present` esegue il commit; registrando il callback prima
del present si ottiene un wake-up al vsync successivo. A quel punto
`CompositorHandler::frame` azzera `frame_pending` e marca la superficie.

Quando un'animazione finisce (`animating == false`), non viene richiesto un nuovo
callback: la shell torna in sleep. Il callback richiesto prima dell'ultimo present
scatta una volta, non trova superfici dirty e non ridisegna. Un evento esterno (IPC, servizio, input)
imposta `force_update` e ridisegna un singolo frame.

`PresentMode::Fifo` è una seconda cintura: anche se un evento esterno tenta di
disegnare più spesso, la presentazione è limitata al refresh e non si accumula
latenza illimitata.

### 3.2 Perché non c'è un timer a 16 ms

Un timer periodico sveglia la CPU anche quando nulla cambia. Qui il timer più
frequente in idle è quello dell'orologio: **una volta al minuto**. Le scadenze
OSD/toast sono timer one-shot esatti. Le animazioni sono guidate dai frame
callback del compositore.

## 4. Superfici e ordine di distruzione

`SurfaceEntry` dichiara i campi in quest'ordine:

```rust
struct SurfaceEntry {
    render: Option<RenderSurface>,   // distrutto per primo
    effect: Option<ExtBackgroundEffectSurfaceV1>,
    layer: Option<LayerSurface>,
    lock: Option<SessionLockSurface>,
    wl_surface: WlSurface,
    ...
}
```

Rust droppa i campi nell'ordine di dichiarazione. La swapchain wgpu deve morire
prima del `wl_surface` a cui è agganciata, altrimenti il backend può usare un
handle Wayland già liberato.

Le superfici sono indicizzate per `ObjectId` del `wl_surface`; gli eventi SCTK
arrivano con il proxy e vengono risolti con `surface.id()`.

### 4.1 Overlay dinamici

Launcher, control center, OSD e toast non esistono quando non servono. Vengono
creati alla prima necessità e distrutti quando la loro animazione di uscita è
finita (`progress < 0.01`). Questo evita swapchain fullscreen permanenti.

La barra invece esiste per output. Il launcher/CC sono fullscreen (scrim + card):
un solo layer surface cattura click fuori dalla card e gestisce il dimming.

### 4.1.1 Avvio robusto (socket scoperti)

La shell parte spesso da systemd, il cui ambiente non contiene né
`WAYLAND_DISPLAY` né un `NIRI_SOCKET` aggiornato: niri, quando il nome semplice
è occupato, appende il proprio pid al socket (`niri.wayland-1.1234.sock`) e
ogni riavvio cambia il percorso. All'avvio `connect_wayland()` prova prima
l'ambiente e poi il socket `wayland-<n>` più recente in `XDG_RUNTIME_DIR`; una
volta connesso esporta `WAYLAND_DISPLAY` così wallpaper e processi figli lo
ereditano. `lard_services::niri::socket_path()` fa lo stesso per niri e
`export_socket_env()` lo rende disponibile ai comandi `niri msg` lanciati dalla
UI. Le connessioni successive ricontrollano il percorso: una shell sopravvive
al riavvio del compositore invece di restare senza barra (era questo il motivo
per cui le icone del launcher "sparivano" dopo un reboot).


### 4.2 Scala e buffer

`wl_surface.set_buffer_scale(scale)` viene impostato prima del commit iniziale.
Il configure di layer/lock è in coordinate **surface-local** (logiche); la
swapchain viene configurata in pixel fisici (`logical * scale`). Lo scene usa
pixel logici; il renderer moltiplica posizioni e raggi per `scale`. Il testo viene
shapato direttamente alla dimensione fisica per evitare blur da upscaling.

La scala frazionaria (`wp_fractional_scale_v1`) non è ancora gestita: niri scala
il buffer intero.

## 5. Renderer

### 5.1 Pipeline

Due pipeline per formato di superficie:

1. **SDF rect**: un buffer di istanze da 80 byte. Ogni istanza ha rettangolo,
   colore lineare straight, 4 raggi, `(border_width, softness, opacity, mode)` e
   colore bordo. Il vertex shader genera i 6 vertici del quad da `vertex_index`:
   niente vertex buffer per la geometria. Il fragment calcola il signed distance
   field di un rounded box con raggi per angolo, fa antialiasing con `fwidth`, e
   gestisce tre modi:
   - forma (fill + bordo, composizione premultiplied);
   - ombra (alpha pieno dentro, falloff `smoothstep` fuori);
   - glow radiale (ellisse con falloff normalizzato, per wallpaper e blob).
2. **Testo**: un buffer di istanze da 48 byte (rect, UV, colore), un atlas R8
   2048² e sampler lineare. Il fragment moltiplica la coverage del glifo per il
   colore e restituisce premultiplied alpha.

Solo due draw call per superficie, indipendentemente dal numero di widget.

### 5.2 Colore

I colori di config sono sRGB straight-alpha. Il renderer li converte in **lineare
straight** e lo shader premultiplice dopo aver composto fill e bordo. La superficie
è `*Srgb` con `CompositeAlphaMode::PreMultiplied`: il compositing del compositore
è corretto e le ombre non hanno aloni.

### 5.3 Testo

- `cosmic-text` fa shaping (harfbuzz/harfrust) e rasterizzazione (swash).
- Layout cache: hash FNV-1a di `(testo, famiglia, dimensione fisica, peso,
  max_width)` + confronto completo. Hit = zero shaping.
- La cache conserva al massimo 512 layout; poi si svuota (l'atlas resta valido).
- Atlas a scaffali con 1 px di padding, upload immediato `queue.write_texture`.
- Glifi `Mask` supportati; `Color` (emoji) e `SubpixelMask` vengono saltati.
- Il testo è posizionato in pixel fisici; l'allineamento viene applicato al
  momento del batch usando la larghezza misurata.
- La shell non ha un contesto GPU, ma per dimensionare i pannelli intorno alle
  etichette usa le **advance reali** del font: `lard_render::text::sans_advances()`
  misura una volta per volta l'ASCII stampabile con Inter e restituisce le
  larghezze per un font da 100 px. `widgets::approx_width` usa quella tabella
  (con una stima grossolana solo per gli script non latini): la stima a
  carattere era troppo ottimista e tagliava testi che entravano, costringendo a
  un fattore di sicurezza del 45%.

### 5.4 Icone delle applicazioni

Il `Icon=` dei file `.desktop` viene risolto sui theme freedesktop
(`hicolor`, `Adwaita`, …), su `~/.icons`, su `/usr/share/pixmaps` e sugli
export Flatpak (`/var/lib/flatpak/exports/share/icons`). I raster passano da
`image`, gli SVG vengono rasterizzati con `resvg` e un-premultiplicati. Ogni
icona occupa una cella 128x128 dell'atlas immagini (gli id 0..119 restano
al carosello dei wallpaper), così il launcher e il dock mostrano l'icona
vera dell'applicazione. Se il file non esiste o non è decodificabile si
rende il glifo Material simbolico.

### 5.5 Isola centrale e notifiche

L'isola centrale della barra è un solo slot: il clock, la data, la campanella e
l'HUD volume/luminosità si sostituiscono a vicenda. Quando arriva una
notifica, l'isola viene **soppressa** (l'orologio scompare) e la notifica viene
disegnata su una **superficie overlay separata** (`lard-notification`,
layer Overlay): così l'isola può allargarsi anche sopra le finestre senza
ridimensionare la superficie della barra né quella delle finestre. La
notifica ha una finestra di 6 s e un decay lineare di 1,4 s alla fine, poi
l'orologio torna. L'isola della notifica è una superficie separata: quando il
messaggio ha un corpo cresce di 12 px oltre l'altezza della barra (e la
superficie viene ridimensionata), così le due righe non toccano i bordi
superiore e inferiore.

### 5.6 Morph dei pannelli

Ogni pannello che si apre da un'isola conosce il rettangolo dell'isola di
partenza (`BarIsland`): il `Popover` e il `Launcher` interpolano la forma fra
il rettangolo dell'isola e quello finale, mentre il contenuto (testi, tile,
azioni) viene disegnato nella posizione finale e fatto apparire con un alpha
ritardato. La barra, dal canto suo, sfuma l'isola che stamorphando
(`Scene::fade_since` scala l'alpha dei comandi disegnati per quell'isola),
così il pannello sembra nascere dall'isola e non davanti ad essa.

La forma non "scivola" durante il morph: l'interpolazione **ancora un angolo
dell'isola** (`widgets::morph_rect` con `MorphAnchor::Left|Center|Right` a
seconda dell'isola da cui nasce) e fa solo crescere il rettangolo. Per il
centro di controllo l'angolo ancorato è quello in alto a destra, quindi il
bordo destro del pannello resta **incollato** al bordo destro dell'isola per
tutta l'animazione. Inoltre il pannello **prende il posto dell'isola**: la sua
superficie parte dalla fila della barra (`y = 0`) e cresce verso il basso, mentre
i controlli restano disposti sotto la barra (`Popover::content_rect`). La forma
raggiunge la misura finale presto (`smoothstep(0.0, 0.62, p)`) e il contenuto
compare dopo (`smoothstep(0.82, 1.0, p)`): quando il testo appare il pannello è
già al suo posto, quindi non esce mai fuori dal rettangolo.

La chiusura è **l'inverso dell'apertura**: `close()` non azzera più l'origine
del morph, il contenuto sfuma per primo e poi la forma si ritira nell'isola
lungo la stessa curva, mentre la barra riaccende l'isola con lo stesso fade.
L'origine viene dimenticata solo quando la molla è tornata a zero. Lo stesso
vale per il launcher (che conserva la sua origine) e per i pannelli che
animano direttamente il proprio `progress` (impostazioni, wallpaper,
control center): nessuno fa snap a zero, così ogni uscita ripercorre al
contrario l'ingresso.

### 5.6.1 Launcher: ricerca, calcolatrice, file finder

Il launcher ha tre sorgenti di righe, tutte nella stessa lista navigabile:
applicazioni (fuzzy `nucleo-matcher`), **calcolatrice** e **file**. La
calcolatrice riconosce un'espressione aritmetica pura (`+ - * / % ^`,
parentesi, decimali) e mostra il risultato come prima riga: `↵` lo copia negli
appunti (`wl-copy`). Le query che sembrano un percorso (`~`, `/`, `./`, `../`)
elencano subito la directory (una sola `read_dir`, con i file nascosti solo se
la query inizia con un punto); le altre parole, quando non trovano
applicazioni, avviano in un thread separato una ricerca per nome nella home
(profondità 4, budget di 6000 voci, cartelle pesanti escluse) e le righe
arrivano appena pronte. La card ha un'altezza massima: le righe che non ci
stanno si raggiungono con la rotella (o con le frecce, che tengono la
selezione visibile) e una scrollbar sottile mostra la posizione.


### 5.7 Raggi e catena

I raggi non sono scritti a mano nelle viste: il tema espone tre token
(`radius.control()` per i controlli piccoli, `radius.card()` per card e tile,
`radius.panel()` per i pannelli) e un flag `radius.linked`. Con la catena
attiva ogni token restituisce il valore master `radius.md`: i pannelli, le
card, i pulsanti, le pastiglie e gli switch seguono tutti lo stesso raggio.
Il renderer taglia sempre il raggio a `min(half_size)`, quindi i valori grandi
sui piccoli elementi restano rotondi.

### 5.8 Icone della barra

Le icone delle isole non vengono dal font Material: sono **disegnate con le
primitive SDF** (rettangoli arrotondati) in `lard-shell/src/bar_icons.rs`, così
sono piene, coerenti con lo stile arrotondato della shell e nitide a qualunque
scala: griglia 2×2 per il launcher, tre slider per il centro di controllo,
campanella per le notifiche.

### 5.9 Backdrop blur

Il blur vero del contenuto dietro una superficie non è ottenibile con wgpu da
solo. Usiamo il protocollo **`ext-background-effect-v1`**: il client dichiara una
regione e il compositore applica il proprio blur. SCTK 0.21 espone già lo stato e
l'oggetto per superficie. Se il global non c'è, la regione non viene impostata e
la UI usa i suoi colori alpha/vetro.

La regione non è mai un rettangolo pieno: ogni pannello dichiara le proprie
aree arrotondate (`wl_region` con bande orizzontali per i raggi). La barra
espone i rettangoli delle **isole** (sinistra, centro, destra) e il compositor
sfoca solo quei pannelli, anche se la superficie della barra copre tutta la
larghezza dell'output. Lo stesso vale per il dock e per gli overlay
(launcher, control center, impostazioni, popover, wallpaper). Il raggio usato
per la regione è quello configurato per le isole (`bar.corner_radius`), quindi
il ritaglio del blur segue sempre la forma disegnata. Opacità e "livello vetro"
(`theme.overlay_opacity`, `theme.frost`) decidono quanto il blur si vede
attraverso i pannelli: il "livello vetro" rende il fondo del pannello più
trasparente (`widgets::panel_alpha`), altrimenti il colore del pannello
nasconderebbe lo sfoco.

La regione segue la **forma corrente** del pannello (`Shell::overlay_rect` usa
`shape_rect()`), non il rettangolo finale: durante il morph il blur cresce
insieme alla forma, senza anticipare un rettangolo grigio nella posizione
definitiva.

L'alternativa (screencopy + blur in GPU) comporterebbe copiare il frame intero a
ogni frame: esclusa per il vincolo di consumo.

## 6. Animazioni

`Spring` risolve in forma chiusa:

```
x'' + 2ζω x' + ω² x = 0
ω = sqrt(k/m),  ζ = c / (2 sqrt(k m))
```

Tre casi (under/critical/over-damped) con soluzioni esponenziali/sinusoidali. Il
valore e la velocità vengono aggiornati in un colpo solo con il `dt` reale, quindi
60 e 120 Hz producono la stessa traiettoria (test in `lard-core`). Il retarget
conserva la velocità: interrompere un'animazione non la fa ripartire da zero.

`FrameClock` clampa `dt` a 100 ms (sospensione/stop del debugger) e mantiene una
EMA di FPS per diagnostica.

## 7. Servizi

Tutti i servizi scrivono su un `calloop::channel::Sender<ServiceEvent>`; il thread
UI non fa mai I/O bloccante.

| Servizio | Meccanismo | Frequenza |
|---|---|---|
| niri | `niri-ipc` `EventStream` + `EventStreamState` | event-driven |
| audio | `pactl subscribe`, coalescing 35 ms, `wpctl`/`pactl` per lo stato | event-driven |
| batteria | sysfs (`/sys/class/power_supply`) | 30 s |
| luminosità | sysfs backlight (+ `brightnessctl` fallback) | 5 s |
| rete | zbus NetworkManager property streams | event-driven (fallback `nmcli` 15 s) |
| bluetooth | zbus BlueZ Adapter1 property streams | event-driven (fallback `bluetoothctl` 30 s) |
| notifiche | zbus, `org.freedesktop.Notifications` su session bus | event-driven |

Le richieste niri (focus workspace, spawn) usano una seconda connessione in un
thread dedicato: il socket event-stream è read-only dopo `EventStream`.

I comandi esterni usano **argv fissi**, mai una shell. `LC_ALL=C` evita parsing
localizzato.

Le notifiche: il daemon assegna gli id e inoltra al UI thread. Il UI possiede
timeout e invia `DaemonCommand::Close { id, reason }`; il daemon emette
`NotificationClosed` una sola volta con il reason corretto (1 expired, 2 dismissed,
3 call). Le capability includono `body` e `actions`; le azioni vengono mostrate
come pulsanti e `ActionInvoked` viene emesso dal daemon.

## 8. PAM, lockscreen, greetd

Due flussi distinti, di proposito:

### Lock (utente già autenticato)

```
lard lock
  → ext-session-lock-v1
  → PAM pam_authenticate (in-process, utente corrente)
  → fade card
  → ext_session_lock_v1.unlock_and_destroy
```

PAM è chiamato in un thread worker. Il modulo FFI dichiara solo:

```c
pam_start, pam_authenticate, pam_end, pam_strerror
```

La conversazione:

- alloca l'array con `calloc` e le risposte con `malloc` (PAM le libera con `free`);
- risponde con password/username ai prompt echo-off/echo-on;
- non fa panic attraverso il confine FFI;
- su errore libera tutto e restituisce `PAM_CONV_ERR`;
- la copia C della password viene azzerata con scritture volatile.

**Niente `pam_acct_mgmt`**: un lock screen non è un login. Su `/etc/shadow`
standard l'utente non può leggere la scadenza password; `pam_unix(account)`
fallirebbe e impedirebbe lo sblocco. La policy di account resta a greetd/login.

La superficie di lock resta **opaca** durante il fade: si dissolve solo la card.
Mostrare il desktop prima di `unlock()` sarebbe una fuga di contenuto.

### Login (greetd)

```
greetd (root) ── PAM + sessione ──► niri-session
     ▲
     │ JSON su $GREETD_SOCK
lard greeter (utente greeter, dentro sway/niri)
```

Il greeter non chiama `setuid` e non avvia processi di sessione: non è un display
manager privilegiato. Questo è il modello corretto e auditabile; un "greeter
standalone" richiederebbe un compositore incorporato e privilegi, fuori scope per
la 0.1.

**Formato IPC.** `greetd` non usa JSON delimitato da newline: ogni messaggio è
`len: u32` (native-endian) seguito dal payload JSON (`Request`/`Response` di
`greetd_ipc`). Un framing sbagliato fa chiudere la connessione con
`client loop failed: i/o error: early eof` e il login non parte mai. Il client
(`lard-greeter/src/greetd.rs`) applica il framing corretto, rifiuta frame oltre
1 MiB e ha un test con un server finto che verifica l'intera sequenza
`create_session` → password → `start_session`.

**Ciclo di vita.** Dopo `start_session` greetd dà al greeter 5 s per uscire
(altrimenti `term`, poi `kill`) e solo allora avvia la sessione: il greeter deve
quindi terminare in fretta. `is_finished` richiede il fade a zero, con un
watchdog a 3 s che chiude comunque il processo se la superficie smette di
presentare frame (output spento), così greetd non resta mai in attesa.

## 9. Input

- Tastiera: SCTK `KeyboardHandler`. `event.utf8` per il testo, keysym per i tasti
  speciali, `Modifiers` per ignorare Ctrl/Alt/Super nei campi di testo.
- Pointer: `PointerHandler` con route per `wl_surface`; coordinate surface-local
  logiche. Ogni lock/greeter surface conserva la propria hit-map, quindi il
  multi-monitor non dipende dall'ultima superficie costruita.
- Il focus tastiera viene tracciato da `KeyboardHandler::enter/leave`. Le
  superfici launcher/CC/greeter usano `KeyboardInteractivity::Exclusive`.
- Il greeter non ha dirty-tracking proprio: il loop è event-driven, quindi ogni
  tick forzato (tasto, click, risultato auth, orologio) marca le sue superfici
  dirty. Il caret arma una deadline a 0.53 s (`GreeterView::next_deadline`):
  senza di essa il repaint arriverebbe solo al tick dell'orologio (60 s) e la
  password digitata sembrerebbe non comparire.
- La barra ha `KeyboardInteractivity::None`.
- L'OSD ha una input region vuota: non intercetta click.
- Click fuori dalla card del launcher/CC chiude l'overlay (scrim fullscreen).

## 10. Config

`config.toml` è deserializzato con `serde` + `toml`, `deny_unknown_fields` per
intercettare i refusi. `Config::validate()` controlla dimensioni, alpha, spring e
nome del servizio PAM. I default sono nel codice e il file di esempio è copiato in
`config/config.toml`; un test parsa e valida il file spedito.

Il caricamento avviene all'avvio. `LARD_CONFIG` permette di puntare a un
file esplicito (utile per l'utente `greeter`). Un watcher inotify sorveglia le
directory di config e richiede `lard ipc reload` dopo un salvataggio
(debounce 350 ms); il reload ricrea renderer/font e superfici della shell, mentre
lo stato (notifiche, launcher, snapshot niri) resta in memoria. La configurazione
delle molle dei singoli widget resta quella dell'avvio.

## 11. Unsafe e memoria

L'intero progetto è safe Rust tranne:

1. **Creazione della superficie wgpu da handle Wayland** (`lard-render/src/gpu.rs`,
   `surface.rs`): i puntatori sono ottenuti da `wl_display`/`wl_surface` vivi; il
   contratto è documentato e l'ordine di drop lo rispetta.
2. **FFI PAM** (`lard-greeter/src/pam.rs`): layout C verificato a mano, nessun
   bindgen, allocazione/deallocazione simmetrica, niente unwind.
3. **Zeroizzazione volatile** della password e dei buffer.

Non ci sono `unsafe` nel renderer, nel layout, nei widget o nei servizi.

Allocazioni: dopo il warm-up il frame loop riusa `Scene` (rect + arena + testi),
i buffer istanziati e la cache layout. Le uniche allocazioni residue sono:
creazione/distruzione di superfici, cambio di stringhe (orologio, titolo) e
`format!`/`truncate` per le etichette nelle viste. Il percorso GPU (istanze,
uniform, atlas, command buffer) non alloca per-frame; quando lo stato è fermo non
viene nemmeno costruita una scena.

## 12. Test

- `lard-core`: parsing colori, spring frame-rate independent, retarget con momentum,
  Bézier, parser `.desktop`/`Exec`, validazione config, config di esempio.
- `lard-greeter`: validazione service/username, mapping errori PAM; link reale a
  `libpam` (senza autenticare).
- `cargo check --workspace` e `cargo build -p lard` verificati.

Manca un test end-to-end con compositore headless; è in roadmap.

## 13. Scelte future

- Atlas RGBA separato per emoji/icone, senza toccare la pipeline R8.
- Hit-map per superficie per lock/greeter multi-output.
- Client nativi PipeWire/NM/BlueZ con zbus, eliminando i subprocess.
- SNI tray.
- Embedded compositor opzionale per un greeter veramente standalone, mantenendo
  greetd come backend di autenticazione/sessione.
