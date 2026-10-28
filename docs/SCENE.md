# Screensaver scenes (Rhai)

User scenes live in `~/.config/lard/scenes/*.rhai` (the folder is created on
first start with two examples). Every file is an independent scene and appears
after the built-in ones: `←`/`→` reach it after the four built-in scenes, or it
can be chosen as "default" by setting `random = false` and using the arrows.

Script scenes draw over the backdrop (a dark gradient) and under the bright dust
layer, which stays active on every scene.

## Contract

```rhai
fn name() { "My scene" }             // optional: label shown in the UI

fn draw(t, w, h) {                   // t in seconds, w/h in pixels
    [ /* commands */ ]
}
```

`draw` is called every frame and must return an **array of maps**. Common fields:

| field   | type   | meaning |
|---------|--------|---------|
| `kind`  | string | `"disc"` (default), `"ring"`, `"rect"` |
| `x`, `y`| float  | position (centre for `disc`/`ring`, corner for `rect`) |
| `r`     | float  | radius for `disc` and `ring` |
| `w`, `h`| float  | size for `rect` |
| `width` | float  | border thickness for `ring` |
| `radius`| float  | corner radius for `rect` |
| `color` | string | `accent`, `text`, `muted`, `danger`, `warning`, `success`, `white`, or `#RRGGBB` / `#RRGGBBAA` |
| `alpha` | float  | 0..1, multiplied by the entrance fade |

Limits: at most 4000 commands per frame, 200 000 operations per call, no file
system or network access (sandboxed engine, no modules). A syntax or runtime
error is logged once and the scene is skipped without stopping the shell.

## Examples

`~/.config/lard/scenes/orbite.rhai` and `nebulosa.rhai` ship with the shell:
copy them, rename the files and edit the values. The script is recompiled when
the file changes, so iterating is instant.

## Colours

The palette names resolve to the active theme, so a scene follows the light or
dark appearance of the shell. `#RRGGBBAA` is useful for glows that must stay
subtle.
