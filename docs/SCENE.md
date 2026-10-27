# Scene del salvaschermo (Rhai)

Le scene scritte dall'utente stanno in `~/.config/lard/scenes/*.rhai` (la
cartella viene creata al primo avvio con due esempi). Ogni file è una scena
indipendente e compare in fondo alla lista degli scenari: con `←`/`→` ci si
arriva dopo le sei scene integrate, oppure la si può scegliere come
"predefinita" mettendo `random = false` e usando le frecce.

Le scene da script disegnano sopra il fondale (un gradiente scuro) e sotto lo
strato di polvere luminosa, che resta attivo su tutte le scene.

## Contratto

```rhai
fn name() { "Il mio nome" }          // opzionale: etichetta mostrata

fn draw(t, w, h) {                   // t in secondi, w/h in pixel
    [ /* comandi */ ]
}
```

`draw` viene chiamata a ogni frame e deve restituire un **array di mappe**.
Campi comuni:

| campo   | tipo   | significato |
|---------|--------|-------------|
| `kind`  | string | `"disc"` (default), `"ring"`, `"rect"` |
| `x`, `y`| float  | posizione (per `disc`/`ring` è il centro, per `rect` l'angolo) |
| `r`     | float  | raggio per `disc` e `ring` |
| `w`, `h`| float  | dimensioni per `rect` |
| `width` | float  | spessore del bordo per `ring` |
| `radius`| float  | angoli per `rect` |
| `color` | string | `accent`, `text`, `muted`, `danger`, `warning`, `success`, `white`, oppure `#RRGGBB` / `#RRGGBBAA` |
| `alpha` | float  | 0..1, moltiplicato per la dissolvenza dell'ingresso |

Limiti: massimo 4000 comandi per frame, 200 000 operazioni per chiamata,
niente accesso al file system o alla rete (engine sandboxato, nessun modulo).
Un errore di sintassi o di esecuzione viene loggato una volta e la scena viene
saltata senza fermare la shell.

## Esempio minimo

```rhai
fn name() { "Orbite" }

fn draw(t, w, h) {
    let out = [];
    for i in 0..12 {
        let a = t * 0.4 + i * 0.523;
        out.push(#{
            kind: "disc",
            x: w / 2.0 + cos(a) * 220.0,
            y: h / 2.0 + sin(a) * 120.0,
            r: 18.0,
            color: "accent",
            alpha: 0.3,
        });
    }
    out
}
```

Modificando un file mentre il salvaschermo è aperto, la cartella viene
riscansita ogni 3 secondi.
