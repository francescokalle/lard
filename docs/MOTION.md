# Motore di animazioni

Tutto ciò che si muove in LARD — barra, popover, launcher, OSD, notifiche, lock
screen — passa da `lard-core/src/anim.rs`. Non è un insieme di `lerp` con una
durata: è un **oscillatore armonico smorzato risolto in forma chiusa**.

```
x(t) = target + e^(-ζωt) · (x₀·cos(ω_d t) + b·sin(ω_d t))
```

Conseguenze pratiche:

- **stessa traiettoria a 30, 60, 120 e 144 Hz**: nessun accumulatore a passo
  fisso, nessun `dt` da tarare;
- **non esplode** dopo uno stallo (sospensione, hiccup del compositor): il `dt`
  viene limitato a 100 ms e la soluzione resta valida;
- **si interrompe senza strappi**: cambiare target a metà volo conserva
  posizione *e* velocità, quindi il movimento curva invece di ripartire da zero;
- **finisce esattamente sul target** (soglie di riposo in distanza e velocità) e
  smette di chiedere frame.

## Descrivere una molla

Due vocabolari per la stessa cosa. Quello del config è fisico, quello del codice
è quello dei designer (modello Apple):

| config (`config.toml`) | codice |
| --- | --- |
| `stiffness`, `damping`, `mass` | `SpringConfig::from_response(response, damping_ratio)` |
| — | `response()` = 2π/ω, durata di un'oscillazione in secondi |
| — | `damping_ratio()` = ζ, 1.0 = nessun rimbalzo |

`ζ < 1` rimbalza (`ζ 0.65` ≈ 7 % di overshoot), `ζ = 1` è critico, `ζ > 1` arriva
lentamente senza mai superare il target. La durata si regola con `response`.

## Preset

| preset | response | ζ | overshoot | si ferma in | frame @120 Hz |
| --- | --- | --- | --- | --- | --- |
| `STRICT` | 0.20 s | 1.00 | 0 % | 0.19 s | 23 |
| `SNAPPY` | 0.30 s | 0.90 | 0.2 % | 0.24 s | 29 |
| `SMOOTH` | 0.45 s | 1.00 | 0 % | 0.35 s | 43 |
| `BOUNCY` | 0.45 s | 0.65 | 6.8 % | 0.36 s | 44 |
| `GENTLE` | 0.60 s | 1.00 | 0 % | 0.45 s | 55 |
| `GLIDE` | 0.75 s | 0.85 | 0.6 % | 0.47 s | 57 |

Numeri generati da `cargo run --release --example motion -p lard-core`, che
scrive anche `docs/motion-springs.svg` (posizione e velocità nel tempo).

A cosa sono associati, nei default:

- `animation.panel` → `SMOOTH` (popover, control center: planata breve, senza rimbalzo);
- `animation.launcher` → `SMOOTH` (planata più lunga, la card è grande);
- `animation.osd` → `STRICT` (volume/luminosità devono rispondere subito);
- `animation.notification` → `BOUNCY` (entrano con un rimbalzo del 7 %).

## Uscita più rapida dell'ingresso

Una chiusura deve *leggere* come più veloce dell'apertura (linee guida Apple):
il launcher chiude con `response × 0.8` mantenendo la stessa forma, poi riapre
con il preset configurato.

## API

```rust
use lard_core::anim::{Spring, SpringConfig, Springs, project_velocity};

// Un valore che inseguе un target.
let mut x = Spring::new(0.0, SpringConfig::SNAPPY);
x.set_target(1.0);
x.update(dt);                 // -> true finché si muove
x.value();                    // posizione
x.fraction(0.0);              // 0..1 per morph/scrim, già clampato

// Interruzione con continuità: nessun salto.
x.set_target(0.0);

// Rilascio di un gesto: la velocità del dito diventa la velocità iniziale.
x.set_target_with_velocity(1.0, finger_px_per_second);

// Proiezione di un fling (dove si fermerebbe).
let landing = project_velocity(finger_px_per_second, 8.0);

// Più canali insieme (posizione + scala, oppure RGBA).
let mut t = Springs::<3>::new([0.0, 0.0, 1.0], SpringConfig::BOUNCY);
t.set_target([10.0, 20.0, 1.1]);
```

Per chi preferisce un solo numero (durata), `CubicBezier` resta disponibile per
gli elementi che non sono "fisici" (scrim, gradienti di sfondo).

## Ridurre il movimento

`theme.animation.reduce_motion = true` fa sì che ogni widget chiami `snap_to`
invece di animare, e `SpringConfig::reduced()` restituisce una molla
praticamente istantanea per chi costruisce l'animazione a mano.
