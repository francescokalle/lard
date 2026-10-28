# Architecture

This document explains *why* the code is split this way and which invariants
keep the 120 Hz loop together. The code is the source of truth; here are the
decisions and the delicate parts.

## 1. Goals and non-goals

Goals:

- one language (Rust) and one process for the shell;
- direct hardware-accelerated rendering, no toolkit;
- idle at ~zero cost, animations independent of the refresh rate;
- lock/greeter separate and unprivileged;
- configuration without recompiling.

Non-goals for 0.1:

- reimplementing a Wayland compositor (the greeter uses a host compositor);
- being a DankLinux/Noctalia clone at the plugin-ecosystem level;
- supporting X11.

## 2. Crates and dependency direction

```
                    ┌──────────────┐
                    │  lard-core   │  config, colors, geometry, springs, desktop
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
              │ lard-wayland│  (Wayland client + loop + input + IPC)
              └─────┬───────┘
                    ▼
              ┌───────────┐
              │   lard    │  binary/CLI
              └───────────┘
```

- `lard-core` knows nothing about Wayland, the GPU or threads.
- `lard-render` knows neither niri nor widgets.
- `lard-services` knows nothing about pixels.
- `lard-wayland` is the only crate that touches `wl_surface` and `wgpu::Surface`.
- `lard-shell`/`lard-greeter` only produce a `Scene` (a draw list).

## 3. Event loop

`calloop` owns the loop. Registered sources:

1. `WaylandSource` — compositor events, configure, input, frame callbacks.
2. `calloop::channel` from the services — niri state, volume, battery, network, BT, notifications.
3. `calloop::channel` from authentication — PAM/greetd result from the worker.
4. `calloop::channel` from IPC — commands from `lard ipc`.
5. Clock `Timer` — aligned to the next minute (or second if the format has `%S`).
6. `Timer` for the next OSD/toast deadline — a single timer, re-armed as needed.
7. Unix socket in a dedicated thread (`$XDG_RUNTIME_DIR/lard.sock`).

The loop calls `dispatch(None)` and **blocks** when there is nothing to do.

### 3.1 Frame scheduling

Per-surface state: `needs_redraw`, `configured`, `frame_pending`.
Global state: `animating`, `force_update`, `callback_fired`.

After every dispatch:

```
tick = callback_fired || force_update
if tick:
    dt = frame_clock.tick()            # clamp 0..100 ms
    animating = shell.update(dt) | greeter.update(dt)
drain_actions()
sync_surfaces()                        # create/destroy overlays
if shell.dirty || animating: mark_all_dirty()
arm_next_deadline()

for each surface with needs_redraw && configured && !frame_pending:
    wl_surface.frame(...)              # callback BEFORE the commit
    draw(); needs_redraw = false

if animating: mark_all_dirty()         # the next callback redraws
```

Why `frame()` before present: the Wayland callback fires on the next commit.
`wgpu::Queue::present` performs the commit; registering the callback before the
present gives a wake-up on the next vsync. At that point
`CompositorHandler::frame` clears `frame_pending` and marks the surface.

When an animation ends (`animating == false`) no new callback is requested: the
shell goes back to sleep. The callback requested before the last present fires
once, finds no dirty surfaces and does not redraw. An external event (IPC,
service, input) sets `force_update` and redraws a single frame.

`PresentMode::Fifo` is a second safety belt: even if an external event tries to
draw more often, presentation is capped at the refresh rate and latency does not
pile up.

### 3.2 Why there is no 16 ms timer

A periodic timer wakes the CPU even when nothing changes. Here the most frequent
idle timer is the clock: **once a minute**. OSD/toast deadlines are exact
one-shot timers. Animations are driven by the compositor's frame callbacks.

## 4. Surfaces and drop order

`SurfaceEntry` declares its fields in this order:

```rust
struct SurfaceEntry {
    render: Option<RenderSurface>,   // dropped first
    effect: Option<ExtBackgroundEffectSurfaceV1>,
    layer: Option<LayerSurface>,
    lock: Option<SessionLockSurface>,
    wl_surface: WlSurface,
    ...
}
```

Rust drops fields in declaration order. The wgpu swapchain must die before the
`wl_surface` it is attached to, otherwise the backend may use an already freed
Wayland handle.

Surfaces are indexed by the `wl_surface` `ObjectId`; SCTK events arrive with the
proxy and are resolved with `surface.id()`.

### 4.1 Dynamic overlays

Launcher, control center, OSD and toasts do not exist when they are not needed.
They are created on first use and destroyed when their exit animation is over
(`progress < 0.01`). This avoids permanent full-screen swapchains.

The bar exists per output. Launcher/CC are full-screen (scrim + card): a single
layer surface captures clicks outside the card and handles the dimming.

### 4.1.1 Robust start-up (undiscovered sockets)

The shell often starts from systemd, whose environment has neither
`WAYLAND_DISPLAY` nor an up-to-date `NIRI_SOCKET`: when the plain name is taken,
niri appends its pid to the socket (`niri.wayland-1.1234.sock`) and every
restart changes the path. At start-up `connect_wayland()` tries the environment
first and then the newest `wayland-<n>` socket in `XDG_RUNTIME_DIR`; once
connected it exports `WAYLAND_DISPLAY` so wallpaper and child processes inherit
it. `lard_services::niri::socket_path()` does the same for niri and
`export_socket_env()` makes it available to `niri msg` commands launched from the
UI. Later connections re-check the path: a shell survives a compositor restart
instead of being left without a bar (which is why the launcher icons "vanished"
after a reboot).

### 4.2 Scale and buffers

`wl_surface.set_buffer_scale(scale)` is set before the initial commit. The
layer/lock configure is in **surface-local** (logical) coordinates; the swapchain
is configured in physical pixels (`logical * scale`). The scene uses logical
pixels; the renderer multiplies positions and radii by `scale`. Text is shaped
directly at the physical size to avoid upscaling blur.

Fractional scaling (`wp_fractional_scale_v1`) is not handled yet: niri scales the
whole buffer.

## 5. Renderer

### 5.1 Pipelines

Two pipelines per surface format:

1. **SDF rect**: an instance buffer of 80 bytes. Each instance has a rectangle,
   straight linear color, 4 radii, `(border_width, softness, opacity, mode)` and a
   border color. The vertex shader generates the 6 quad vertices from
   `vertex_index`: no vertex buffer for the geometry. The fragment computes the
   signed distance field of a rounded box with per-corner radii, antialiases with
   `fwidth`, and handles three modes:
   - shape (fill + border, premultiplied composition);
   - shadow (full alpha inside, `smoothstep` falloff outside);
   - radial glow (ellipse with normalized falloff, for wallpaper and blobs).
2. **Text**: an instance buffer of 48 bytes (rect, UV, color), a 2048² R8 atlas
   and a linear sampler. The fragment multiplies the glyph coverage by the color
   and returns premultiplied alpha.

Only two draw calls per surface, regardless of the number of widgets.

### 5.2 Color

Config colors are sRGB straight-alpha. The renderer converts them to **straight
linear** and the shader premultiplies after compositing fill and border. The
surface is `*Srgb` with `CompositeAlphaMode::PreMultiplied`: the compositor's
compositing is correct and shadows have no halos.

### 5.3 Text

- `cosmic-text` does shaping (harfbuzz/harfrust) and rasterization (swash).
- Layout cache: FNV-1a hash of `(text, family, physical size, weight, max_width)`
  + full comparison. Hit = zero shaping.
- The cache keeps at most 512 layouts, then it is cleared (the atlas stays valid).
- Shelf atlas with 1 px padding, immediate upload with `queue.write_texture`.
- `Mask` glyphs are supported; `Color` (emoji) and `SubpixelMask` are skipped.
- Text is positioned in physical pixels; alignment is applied at batch time using
  the measured width.
- The shell has no GPU context, but to size panels around labels it uses the
  font's **real advances**: `lard_render::text::sans_advances()` measures printable
  ASCII once with Inter and returns widths for a 100 px font.
  `widgets::approx_width` uses that table (with a rough estimate only for non-Latin
  scripts): the per-character estimate was too optimistic and clipped text that
  would have fit, forcing a 45% safety factor.

### 5.4 Application icons

The `Icon=` of `.desktop` files is resolved through the freedesktop themes
(`hicolor`, `Adwaita`, …), `~/.icons`, `/usr/share/pixmaps` and the Flatpak
exports (`/var/lib/flatpak/exports/share/icons`). Raster images go through
`image`, SVGs are rasterized with `resvg` and un-premultiplied. Every icon takes a
128x128 cell of the image atlas (ids 0..119 stay with the wallpaper carousel), so
the launcher and the dock show the real application icon. If the file is missing
or cannot be decoded, the symbolic Material glyph is drawn instead.

### 5.5 Centre island and notifications

The centre island of the bar is a single slot: clock, date, bell and the
volume/brightness HUD replace each other. When a notification arrives the island
is **suppressed** (the clock disappears) and the notification is drawn on a
**separate overlay surface** (`lard-notification`, Overlay layer): this way the
island can grow over windows without resizing the bar surface or the window
surfaces. The notification has a 6 s window and a 1.4 s linear decay at the end,
then the clock comes back. The notification island is a separate surface: when the
message has a body it grows 12 px past the bar height (and the surface is
resized), so the two lines never touch the top and bottom edges.

### 5.6 Panel morph

Every panel opened from an island knows the starting island rectangle
(`BarIsland`): `Popover` and `Launcher` interpolate the shape between the island
rectangle and the final one, while the content (text, tiles, actions) is drawn at
its final position and faded in with a delay. The bar, on its side, fades the
island that is morphing (`Scene::fade_since` scales the alpha of the commands
drawn for that island), so the panel seems to be born from the island instead of
in front of it.

The shape does not "slide" during the morph: the interpolation **anchors one
corner of the island** (`widgets::morph_rect` with `MorphAnchor::Left|Center|Right`
depending on the island it comes from) and only grows the rectangle. For the
control center the anchored corner is the top-right one, so the right edge of the
panel stays **glued** to the right edge of the island for the whole animation.
Moreover the panel **takes the island's place**: its surface starts on the bar row
(`y = 0`) and grows downwards, while the controls stay laid out below the bar
(`Popover::content_rect`). The shape reaches its final size early
(`smoothstep(0.0, 0.62, p)`) and the content appears later
(`smoothstep(0.62, 0.95, p)`): when the text appears the panel is already in place,
so it never spills outside the rectangle.

Closing is **the reverse of opening**: `close()` no longer clears the morph
origin, the content fades first and then the shape withdraws into the island along
the same curve, while the bar lights the island up again with the same fade. The
origin is forgotten only when the spring is back to zero. The same holds for the
launcher (which keeps its origin) and for the panels that animate their own
`progress` directly (settings, wallpaper, control center): none of them snaps to
zero, so every exit retraces the entrance backwards.

### 5.6.1 Launcher: search, calculator, file finder

The launcher has three row sources, all in the same navigable list:
applications (fuzzy `nucleo-matcher`), **calculator** and **files**. The
calculator recognises a pure arithmetic expression (`+ - * / % ^`, parentheses,
decimals) and shows the result as the first row: `↵` copies it to the clipboard
(`wl-copy`). Queries that look like a path (`~`, `/`, `./`, `../`) list the
directory immediately (a single `read_dir`, with hidden files only when the query
starts with a dot); the other words, when they match no application, start a
background name search in the home directory (depth 4, budget of 6000 entries,
heavy folders excluded) and the rows arrive as soon as they are ready. The card has
a maximum height: rows that do not fit are reached with the wheel (or with the
arrows, which keep the selection visible) and a thin scrollbar shows the position.

### 5.7 Radii and chain

Radii are not hard-coded in the views: the theme exposes three tokens
(`radius.control()` for small controls, `radius.card()` for cards and tiles,
`radius.panel()` for panels) and a `radius.linked` flag. With the chain on, every
token returns the master value `radius.md`: panels, cards, buttons, pills and
switches all follow the same radius. The renderer always clamps the radius to
`min(half_size)`, so large values on small elements stay round.

### 5.8 Bar icons

The island icons do not come from the Material font: they are **drawn with the SDF
primitives** (rounded rectangles) in `lard-shell/src/bar_icons.rs`, so they are
solid, consistent with the rounded style of the shell and sharp at any scale: a
2×2 grid for the launcher, three sliders for the control center, a bell for
notifications.

### 5.9 Backdrop blur

The real blur of the content behind a surface cannot be obtained with wgpu alone.
We use the **`ext-background-effect-v1`** protocol: the client declares a region
and the compositor applies its own blur. SCTK 0.21 already exposes the state and
the per-surface object. If the global is missing, the region is not set and the UI
uses its alpha/glass colors.

The region is never a full rectangle: every panel declares its own rounded areas
(`wl_region` with horizontal bands for the radii). The bar exposes the **island**
rectangles (left, centre, right) and the compositor blurs only those panels, even
though the bar surface covers the whole output width. The same holds for the dock
and for the overlays (launcher, control center, settings, popover, wallpaper). The
radius used for the region is the configured one for the islands
(`bar.corner_radius`), so the blur clipping always follows the drawn shape.
Opacity and "glass level" (`theme.overlay_opacity`, `theme.frost`) decide how much
of the blur is visible through the panels: the "glass level" makes the panel
background more transparent (`widgets::panel_alpha`), otherwise the panel color
would hide the blur.

The region follows the **current shape** of the panel (`Shell::overlay_rect` uses
`shape_rect()`), not the final rectangle: during the morph the blur grows with the
shape, without anticipating a grey rectangle at the final position.

The alternative (screencopy + GPU blur) would mean copying the whole frame every
frame: excluded because of the power budget.

## 6. Animations

`Spring` is solved in closed form:

```
x'' + 2ζω x' + ω² x = 0
ω = sqrt(k/m),  ζ = c / (2 sqrt(k m))
```

Three cases (under/critical/over-damped) with exponential/sinusoidal solutions.
The value and the velocity are updated in one go with the real `dt`, so 60 and
120 Hz produce the same trajectory (tests in `lard-core`). Retargeting keeps the
velocity: interrupting an animation does not restart it from zero.

`FrameClock` clamps `dt` to 100 ms (suspend/debugger stop) and keeps an EMA of the
FPS for diagnostics.

## 7. Services

All services write to a `calloop::channel::Sender<ServiceEvent>`; the UI thread
never does blocking I/O.

| Service | Mechanism | Rate |
|---|---|---|
| niri | `niri-ipc` `EventStream` + `EventStreamState` | event-driven |
| audio | `pactl subscribe`, 35 ms coalescing, `wpctl`/`pactl` for the state | event-driven |
| battery | sysfs (`/sys/class/power_supply`) | 30 s |
| brightness | sysfs backlight (+ `brightnessctl` fallback) | 5 s |
| network | zbus NetworkManager property streams | event-driven (`nmcli` fallback 15 s) |
| bluetooth | zbus BlueZ Adapter1 property streams | event-driven (`bluetoothctl` fallback 30 s) |
| notifications | zbus, `org.freedesktop.Notifications` on the session bus | event-driven |

niri requests (focus workspace, spawn) use a second connection in a dedicated
thread: the event-stream socket is read-only after `EventStream`.

External commands use **fixed argv**, never a shell. `LC_ALL=C` avoids localized
parsing.

Notifications: the daemon assigns the ids and forwards to the UI thread. The UI
owns timeouts and sends `DaemonCommand::Close { id, reason }`; the daemon emits
`NotificationClosed` once with the right reason (1 expired, 2 dismissed, 3 call).
Capabilities include `body` and `actions`; actions are shown as buttons and
`ActionInvoked` is emitted by the daemon.

## 8. PAM, lockscreen, greetd

Two distinct flows, on purpose:

### Lock (user already authenticated)

```
lard lock
  → ext-session-lock-v1
  → PAM pam_authenticate (in-process, current user)
  → card fade
  → ext_session_lock_v1.unlock_and_destroy
```

PAM is called on a worker thread. The FFI module declares only:

```c
pam_start, pam_authenticate, pam_end, pam_strerror
```

The conversation:

- allocates the array with `calloc` and the responses with `malloc` (PAM frees them with `free`);
- answers password/username to the echo-off/echo-on prompts;
- never panics across the FFI boundary;
- on error frees everything and returns `PAM_CONV_ERR`;
- the C copy of the password is zeroed with volatile writes.

**No `pam_acct_mgmt`**: a lock screen is not a login. On a standard `/etc/shadow`
the user cannot read the password expiry; `pam_unix(account)` would fail and block
the unlock. Account policy stays with greetd/login.

The lock surface stays **opaque** during the fade: only the card dissolves.
Showing the desktop before `unlock()` would leak content.

### Login (greetd)

```
greetd (root) ── PAM + session ──► niri-session
     ▲
     │ JSON over $GREETD_SOCK
lard greeter (user greeter, inside sway/niri)
```

The greeter does not call `setuid` and does not start session processes: it is not
a privileged display manager. This is the correct, auditable model; a
"standalone greeter" would need an embedded, privileged compositor, which is out
of scope for 0.1.

**IPC format.** `greetd` does not use newline-delimited JSON: every message is
`len: u32` (native-endian) followed by the JSON payload (`Request`/`Response` of
`greetd_ipc`). Wrong framing makes the connection close with
`client loop failed: i/o error: early eof` and the login never starts. The client
(`lard-greeter/src/greetd.rs`) applies the correct framing, rejects frames over
1 MiB and has a test with a fake server that checks the whole
`create_session` → password → `start_session` sequence.

**Lifecycle.** After `start_session` greetd gives the greeter 5 s to exit
(otherwise `term`, then `kill`) and only then starts the session: the greeter must
therefore terminate quickly. `is_finished` requires the fade to reach zero, with a
3 s watchdog that kills the process anyway if the surface stops presenting frames
(display off), so greetd never waits forever.

## 9. Input

- Keyboard: SCTK `KeyboardHandler`. `event.utf8` for text, keysyms for special
  keys, `Modifiers` to ignore Ctrl/Alt/Super in text fields.
- Pointer: `PointerHandler` routed per `wl_surface`; logical surface-local
  coordinates. Every lock/greeter surface keeps its own hit-map, so multi-monitor
  does not depend on the last surface built.
- Keyboard focus is tracked by `KeyboardHandler::enter/leave`. Launcher/CC/greeter
  surfaces use `KeyboardInteractivity::Exclusive`.
- The greeter has no dirty-tracking of its own: the loop is event-driven, so every
  forced tick (key, click, auth result, clock) marks its surfaces dirty. The caret
  arms a 0.53 s deadline (`GreeterView::next_deadline`): without it the repaint
  would only arrive at the clock tick (60 s) and the typed password would seem not
  to appear.
- The bar uses `KeyboardInteractivity::None`.
- The OSD has an empty input region: it does not intercept clicks.
- A click outside the launcher/CC card closes the overlay (full-screen scrim).

## 10. Config

`config.toml` is deserialized with `serde` + `toml`, `deny_unknown_fields` to catch
typos. `Config::validate()` checks sizes, alpha, springs and the PAM service name.
Defaults live in the code and the example file is copied to `config/config.toml`;
a test parses and validates the shipped file.

Loading happens at start-up. `LARD_CONFIG` points at an explicit file (useful for
the `greeter` user). An inotify watcher watches the config directories and asks for
`lard ipc reload` after a save (350 ms debounce); the reload recreates the
renderer/fonts and the shell surfaces, while the state (notifications, launcher,
niri snapshot) stays in memory. Per-widget spring settings stay those of the
start-up.

## 11. Unsafe and memory

The whole project is safe Rust except:

1. **Creating the wgpu surface from Wayland handles** (`lard-render/src/gpu.rs`,
   `surface.rs`): the pointers come from live `wl_display`/`wl_surface`; the
   contract is documented and the drop order respects it.
2. **PAM FFI** (`lard-greeter/src/pam.rs`): hand-checked C layout, no bindgen,
   symmetric allocation/deallocation, no unwinding.
3. **Volatile zeroing** of the password and of the buffers.

There is no `unsafe` in the renderer, layout, widgets or services.

Allocations: after warm-up the frame loop reuses `Scene` (rects + arena + texts),
the instance buffers and the layout cache. The only remaining allocations are:
surface creation/destruction, string changes (clock, title) and
`format!`/`truncate` for the labels in the views. The GPU path (instances,
uniforms, atlas, command buffer) does not allocate per frame; when the state is
still, a scene is not even built.

## 12. Tests

- `lard-core`: color parsing, frame-rate independent springs, momentum-preserving
  retarget, Bézier, `.desktop`/`Exec` parsing, config validation, example config.
- `lard-greeter`: service/username validation, PAM error mapping; real link to
  `libpam` (without authenticating).
- `cargo check --workspace` and `cargo build -p lard` verified.

An end-to-end test with a headless compositor is missing; it is on the roadmap.

## 13. Future choices

- A separate RGBA atlas for emoji/icons, without touching the R8 pipeline.
- Per-surface hit-map for multi-output lock/greeter.
- Native PipeWire/NM/BlueZ clients with zbus, removing the subprocesses.
- SNI tray.
- Optional embedded compositor for a truly standalone greeter, keeping greetd as
  the authentication/session backend.
