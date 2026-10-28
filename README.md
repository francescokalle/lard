# LARD

> **LARD** — *light as rust dust*

Native desktop shell for [niri](https://github.com/YaLTeR/niri), written **entirely in Rust**.
No QML, GTK, Qt, Node.js, Electron or QuickShell: only `wayland-client` + `smithay-client-toolkit`,
`wgpu`, `cosmic-text`, `calloop` and `niri-ipc`.

```
bar with 3 islands ─ dock ─ fuzzy launcher ─ OSD ─ notifications ─ control center
                          │
                 PAM lockscreen / greetd greeter
                          │
       wgpu (SDF + glyph atlas) · blur · spring animations · TOML
```

## Install with one command

```bash
curl -fsSL https://lard.francescocallegaro.com/install.sh | bash
```

The script downloads a snapshot of the repository (no `git` needed), installs the
missing build dependencies and the Rust toolchain (pacman, apt, dnf, zypper),
builds in release mode and **enables autostart** (`lard.service` when user systemd
is available, otherwise `spawn-at-startup` in niri — never both). It is a small TUI
with a step per phase; `--no-deps` skips the package installation.

From a local checkout the equivalent is:

```bash
./install.sh            # build, install, enable autostart
```

The script builds in release mode and installs:

- `/usr/local/bin/lard`
- system config `/etc/lard/config.toml` and, if missing, the user config
- `/etc/pam.d/lard-greeter`
- greeter assets (`/etc/lard/greeter-sway.conf`, `greeter.kdl`)
- user systemd unit and documentation

It asks for `sudo` only for system files, never to compile. On a system without
Rust, the stable toolchain is installed via rustup automatically.

Add the niri block to `~/.config/niri/config.kdl` automatically (with backup and
conflict checks):

```bash
./install.sh --niri
```

Rootless, user-only installation (`~/.local/bin`, `~/.config`):

```bash
./install.sh --user
```

Packaging staging (does not touch the system):

```bash
./install.sh --root /tmp/stage --prefix /usr
```

Uninstall (the user config is preserved; use `--purge` to remove it):

```bash
./install.sh --uninstall
```

A block added by `--niri` is not removed automatically (to avoid touching custom
binds); the installer reports it and leaves the backup.

`make install` / `make uninstall` are equivalent. The greetd greeter still
requires an explicit choice of host compositor: the installer ships the assets but
**does not modify** `/etc/greetd/config.toml`, to avoid lockouts. The procedure is
in [docs/greetd/config.toml](docs/greetd/config.toml).

## LARD as the default shell

LARD is meant to replace DankMaterialShell: niri, greetd and the desktop stay
untouched, only the user shell changes. The bundled script handles the service and
the niri config:

```bash
lard-switch status        # service status
lard-switch start|stop|restart
lard-switch prepare       # regenerate and validate ~/.config/niri/config-lard.kdl
lard-switch default-rust  # LARD as the default shell at login
```

The bar has the gear for the settings center; from the keyboard:

```bash
lard ipc settings
```

The niri snippets live in `~/.config/niri/lard` (layout, alttab, output, cursor and
the shell binds) and the alternative config in `~/.config/niri/config-lard.kdl`:
window-management binds stay the ones you already have, while launcher, volume,
brightness, lock and power call `lard`.

The greeter: with `greetd` the default session becomes
`niri --config /etc/greetd/niri/config.kdl`, which hosts `lard greeter` as a
Wayland client (same procedure as [docs/greetd/config.toml](docs/greetd/config.toml)).

## Previews

Rendered offscreen by the same wgpu renderer (`cargo run -p lard-shell --example preview`):

| Bar with three islands | Dock (optional, off by default) |
|---|---|
| ![Bar](preview/bar.png) | ![Dock](preview/dock.png) |

| Launcher | Control center |
|---|---|
| ![Launcher](preview/launcher.png) | ![Control center](preview/control.png) |

| Shell settings | Grouped control center |
|---|---|
| ![Settings](preview/settings.png) | ![Control center popover](preview/control-popover.png) |

| Power from the control center | |
|---|---|
| ![Power](preview/control-popover-power.png) | |

| Lock screen | Greeter |
|---|---|
| ![Lock](preview/lock.png) | ![Greeter](preview/greeter.png) |

| OSD | Notifications with actions |
|---|---|
| ![OSD](preview/osd.png) | ![Toasts](preview/toasts.png) |

> **Status: 0.1 validated on niri 26.04.** Bar/dock/launcher/control center,
> D-Bus notifications, OSD, compositor blur, `ext-session-lock` lockscreen with
> PAM and auto-unlock were tested on real niri (Intel HD 5500, Vulkan).
> The greetd greeter ships as an asset but is **not** enabled automatically: the
> display manager configuration stays the one chosen by the system.

---

## Real status

### Implemented

| Component | Status | Notes |
|---|---|---|
| **Top bar** | ✅ | Three floating Dank-style islands: workspaces + title, clock/date and a single **Controls** button that opens a macOS-style control center. Every panel born from an island (launcher, controls, clock, notifications) **morphs** out of its island: the shape grows **anchored to the island corner** (it does not slide), the island fades and the panel **takes the island's place** in the bar row, growing downwards; content appears once the shape is in place. The panel groups Wi‑Fi, Bluetooth, do-not-disturb, notifications, brightness, volume, settings and power; **CPU, RAM and battery stay in the right island** of the bar (percentages sized for three digits, so `%` never wraps at 100%). The Wi‑Fi/Bluetooth tiles have a toggle and a clickable body; the manage link expands the same popover in place (up to 400×640) with list, scanning, password and scrolling. The Power submenu holds lock, log out, suspend, restart and power off. The right side of the bar has **one button** (the control center) next to CPU and RAM: the launcher is no longer top-left (open it with `Mod+D` or `lard ipc launcher`). Embedded Material icons (and **hand-drawn glyphs** for the islands: control-center sliders, bell), one `Bar` per output. Islands are suspended glass: `ext-background-effect-v1` clips the compositor blur to the three rounded panels (the blurred region **follows the growing shape**), and height, **corner radius**, opacity and island blur are configurable from the settings center. |
| **Dock** | ✅ | Centred bottom island, pinned + running apps, real application icons (Material glyph fallback), running indicators. **Off by default**; re-enable from Settings → Dock or `[dock]`. |
| **Launcher** | ✅ | Full-screen overlay with scrim and blur, `nucleo-matcher` fuzzy search, favourites first, **real application icons** (PNG/JPEG/WebP and SVG rasterized with `resvg`, resolved through freedesktop themes and Flatpak exports), keyboard + mouse, caret, open/close animation (the exit is the reverse of the entrance). The list **scrolls** with the wheel when rows exceed the card height (thin scrollbar). It is also a **calculator** (`+ - * / % ^`, `↵` copies the result) and a **file finder**: paths (`~`, `/`, `./`, `../`) list the directory, other queries search by name in the background. |
| **Volume/brightness OSD** | ✅ | HUD inside the centre island (it replaces clock/date), vertical spring crossfade, auto-hide with an exact timer (no polling). |
| **Brightness** | ✅ | Slider in the control center, OSD and IPC; sysfs + `brightnessctl` fallback. |
| **Notifications** | ✅ | Real `org.freedesktop.Notifications` D-Bus server with `zbus`: timeouts, dismiss, actions (`ActionInvoked`), replacement by `replaces_id`, `NotificationClosed`. The bell sits in the centre island (right of the date, with a badge); the notification **takes the clock's place** and widens the island on a dedicated overlay surface that sits **above windows** without resizing them, with a ~1.4 s decay. Text is sized with the font's **real advances**, so long notifications are never ellipsized. The bell opens the **notification panel under the centre island** with the list and its actions. |
| **Control center** | ✅ | Two entry points to the same engine: the grouped popover of the Controls button (macOS-style grid with tiles, sliders, system status and the Power submenu) and the blurred side panel, still available over IPC. Wi‑Fi and Bluetooth pages with scanning (manual and **automatic every 20 s**, without redraws when the list does not change), network/device selection, **Wi‑Fi password** and **802.1X credentials** (username + password, PEAP/MSCHAPv2 with TTLS/PAP fallback) for **WPA‑Enterprise** networks, connection, Power page (lock, log out, suspend, restart, power off). Volume and brightness sliders are **draggable** (press + move, not just click), update the UI optimistically and coalesce writes so the cursor never lags. Long lists scroll with wheel and arrows. Manage links expand the same popover in place, without opening the side panel. |
| **Light / dark** | ✅ | Three modes — System, Light, Dark — from Settings → Appearance, `lard ipc theme-light|theme-dark|theme-system` or `[theme] mode`. The light palette is derived from the dark one (accent keeps its hue, darkened for contrast) and the choice is **published to the desktop**: `gsettings org.gnome.desktop.interface color-scheme` plus `gtk-application-prefer-dark-theme` in `~/.config/gtk-3.0|gtk-4.0/settings.ini`, so the XDG portal — and with it GTK apps, Firefox/Zen, Chromium and Electron — follow. In System mode a watcher re-reads the preference every 5 s and LARD follows changes made elsewhere. |
| **Settings center** | ✅ | Blurred overlay with Dank/Azure/Green/Pink/Neutral palettes, island height, **radius** and **opacity**, **dock opacity**, toggles for the modules shown in the control center, and an Effects section with **panel blur**, **island blur**, **panel opacity** and **glass level**, plus reduce motion, notification position and diagnostics. Radii have three levels (controls, cards/tiles, panels) and a **chain** button that locks them all to the master value. The niri configuration is edited from the GUI, split by topic: **Displays** (per-monitor resolution and scale, with the monitor list), **Windows** (gaps, borders, focus ring, centre column, tab bar) and **Advanced** (reserved space, rarely needed). Values are written to the KDL files with a `.bak` backup and niri is reloaded. Saves to TOML with a backup and recreates the surfaces. |
| **Lockscreen** | ✅ | `ext-session-lock-v1` + native PAM, correct `ack_configure` before the first buffer, fail-open watchdog, unlock with password. Tested on real niri. |
| **Greeter** | ✅ | Avatar/user/password/session/power UI; **greetd** backend for login. `--preview` mode with PAM to try the UI without starting a session. |
| **Renderer** | ✅ | One shared wgpu device, instanced SDF pipelines (rects, borders, shadows/glow) + R8 glyph atlas, text cache, string arena, `ext-background-effect-v1`. |
| **Animations** | ✅ | Analytic damped-harmonic solver: identical trajectory at 60/120/144 Hz, no accumulator. Spring presets described by `response` + `damping ratio`. |
| **Config** | ✅ | Complete `config.toml` with themes, fonts, radii (controls / cards-tiles / panels, with the **chain** to lock them equal), shadows, blur, spacing and springs; unknown fields are errors. The wallpaper chosen in the carousel is remembered here and re-applied at start-up. With `follow_niri` the shell **does not duplicate** the values niri owns but reads them from its configuration (see below). |
| **Runtime control** | ✅ | `$XDG_RUNTIME_DIR/lard.sock` socket + `lard ipc …` CLI (including `lard ipc controls` for the grouped panel, `lard ipc battery`, `lard ipc system` and `lard ipc settings-<tab>`). |

### Values shared with niri

The shell and niri must not keep two copies of the same colors: with
`theme.follow_niri = true` (default) LARD reads niri's configuration and follows
these values, updating them on every reload:

| LARD value | niri key |
|---|---|
| `theme.background` (color only, transparency stays with LARD) | `layout.background-color` |
| `theme.accent` | `layout.focus-ring.color` |
| `theme.danger` | `layout.border.urgent-color` |

`background-color "transparent"` is ignored (niri shows the wallpaper). The
settings palette no longer touches these three values: change them in niri and
LARD realigns immediately (the file watcher reloads the configuration on the fly).

### Partial / to be completed

| Area | What is missing |
|---|---|
| **System tray (SNI)** | No tray icons: only the built-in status indicators. StatusNotifierItem (DBus + menu + images) is the next module. |
| **Native services** | Network (NetworkManager) and Bluetooth (BlueZ) are event-driven zbus clients with `nmcli`/`bluetoothctl` fallbacks. Audio still uses `pactl subscribe` + `wpctl`. |
| **Blur** | Uses `ext-background-effect-v1` on panels and islands: if niri does not expose it, the UI stays glassy/alpha. The region is clipped to the panels (bar islands, dock, overlays). There is no home-made backdrop blur (it would need screencopy: expensive). |
| **Multi-monitor** | Bars and lock surfaces per output ✅; every lock surface has its own hit-map. The greeter still uses a single output. |
| **Fractional scaling** | Integer scale (`wl_output.scale`). 1.25/1.5x is scaled by the compositor. |
| **Application icons / emoji** | Application icons are the real ones from the icon theme: PNG/JPEG/WebP through `image`, SVG rasterized with `resvg` (resolved in `hicolor`, Adwaita themes and Flatpak exports). XPM/ICO are not supported and colour emoji are rendered as monochrome masks in the R8 atlas (Wi‑Fi names and titles stay readable). |
| **Hot reload config** | ✅: inotify watcher on `config.toml` (and manual `lard ipc reload`). Reloads theme/layout/fonts and recreates the surfaces; per-widget spring parameters stay those of the start-up. |
| **Screensaver** | Built into the shell and fully procedural: a flat dark backdrop, four scenes — Liquid, Mercury, Aurora, Gas — evaluated in the fragment shader, plus a bright dust layer over each, with no images, sprites or particles. Entrance and exit are a burn-in (lights first, then the bodies, the room last) and ← → switch scene with a crossfade, with no automatic slideshow. On top of that, **user-written scenes**: Rhai scripts in `~/.config/lard/scenes/*.rhai` that draw shapes (discs, rings, rectangles) over the procedural room, with two ready examples. Everything is configurable from the settings: idle time, fixed or random scene, script scenes, entrance, glass, speed, intensity, clock. The lock screen also shows the desktop wallpaper (darkened, configurable from Settings → Lock). Idle comes from `ext-idle-notify-v1` (no `swayidle`), the surface is one overlay layer per output and any other key or pointer movement closes it. It does not lock the session: that is `lard lock`. |

---

## Architecture

Cargo workspace with separate responsibilities (no circular dependencies):

```
lard/
├── Cargo.toml
├── install.sh       build + install/uninstall in one command
├── Makefile
├── config/config.toml
├── docs/
├── assets/fonts/    Inter + JetBrains Mono + Material Icons embedded
└── crates/
    ├── lard-core         config, theme, sRGB/linear colors, geometry, springs, .desktop
    ├── lard-render       wgpu device/surface, SDF, text atlas, layout cache, WGSL shaders
    ├── lard-services     niri IPC, audio, battery, network, bluetooth, D-Bus notifications, power
    ├── lard-shell        views: bar, launcher, OSD, toasts, control center
    ├── lard-greeter      PAM FFI, greetd IPC, users/sessions, lockscreen/greeter UI
    ├── lard-wayland      Wayland client, layer-shell/session-lock, input, frame loop, IPC
    └── lard              binary + CLI
```

Dependencies:

```
lard → lard-wayland → { lard-shell, lard-greeter, lard-render, lard-services, lard-core }
lard-shell  → { lard-render, lard-services, lard-core }
lard-greeter→ { lard-render, lard-services, lard-core }
lard-render → lard-core
lard-services → lard-core
```

Diagram and detailed decisions: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

### Wayland surfaces

| Role | Protocol | Layer / anchor | Keyboard |
|---|---|---|---|
| Bar | `zwlr_layer_shell_v1` | Top, TOP+LEFT+RIGHT | None |
| Dock | `zwlr_layer_shell_v1` | Top, BOTTOM | None |
| Launcher | layer shell | Overlay fullscreen | Exclusive |
| Control center | layer shell | Overlay fullscreen | Exclusive |
| OSD | inside the bar | centre island, no dedicated surface | — |
| Toast | layer shell | Overlay, TOP+RIGHT | None |
| Greeter | layer shell | Overlay fullscreen | Exclusive |
| Lockscreen | `ext-session-lock-v1` | one lock surface per output | managed by the compositor |

Every surface has a `SurfaceEntry` with `RenderSurface` declared **before**
`LayerSurface`/`SessionLockSurface`/`wl_surface`: the drop order guarantees the
wgpu swapchain is destroyed before the Wayland object.

---

## Manual build (optional)

The installer does everything; these steps are only for development or packaging.
Requirements: stable Rust (edition 2024, `rust-version = 1.89`), `pkg-config`,
libwayland, libpam, fontconfig, Vulkan or OpenGL.

```bash
# Debian/Ubuntu
sudo apt install build-essential pkg-config libwayland-dev libpam0g-dev \
                 libfontconfig1-dev libxkbcommon-dev libvulkan-dev

# Arch
sudo pacman -S base-devel pkgconf wayland pam fontconfig libxkbcommon vulkan-icd-loader

cargo build --release --locked
cargo run -p lard-render --example headless_smoke   # offscreen GPU/WGSL/font check
cargo run -p lard-shell --example preview           # screenshots in ./preview/
```

The config path is `$LARD_CONFIG` when set, otherwise
`$XDG_CONFIG_HOME/lard/config.toml` (or `~/.config/...`), then
`/etc/lard/config.toml` (installed by the installer, useful for the greeter), then
the built-in defaults.

---

## Running

Inside a niri session:

```bash
lard                 # shell (bar + launcher + OSD + notifications + CC)
lard lock            # lockscreen
lard greeter --preview  # try the greeter UI with PAM, no session started
```

Suggested binds in `~/.config/niri/config.kdl` ([example](docs/niri/config.kdl)):

```kdl
spawn-at-startup "lard"

binds {
    Mod+D       { spawn "lard" "ipc" "launcher"; }
    Mod+Shift+E { spawn "lard" "ipc" "control-center"; }
    Mod+Escape  { spawn "lard" "lock"; }
    Mod+Shift+Q { spawn "lard" "ipc" "quit"; }
}
```

Available IPC:

```
launcher, control-center, controls, notifications, wifi, bluetooth, battery,
system, power-menu, settings, wallpapers, volume-up, volume-down, toggle-mute,
brightness-up, brightness-down, reload, lock, quit
```

---

## Lockscreen and PAM

The lock screen is a **separate, unprivileged process**. `ext-session-lock-v1` is
granted by the compositor; the surface stays opaque until the password is verified,
then the card fades out and `unlock()` is called.

Installing the PAM service:

```bash
sudo install -Dm644 docs/pam.d/lard-greeter /etc/pam.d/lard-greeter
```

The file uses **only** `auth required pam_unix.so`. We do not call `pam_acct_mgmt`:
a lockscreen runs as the user and on a standard `/etc/shadow` it cannot read the
password expiry; `pam_unix(account)` would fail and the lock would stay stuck.
Account policy belongs to the login path (greetd). If you want `pam_faillock`, add
it to the service knowingly and test the unlock.

Implemented security:

- PAM through a minimal FFI (`pam_start`/`pam_authenticate`/`pam_end`), no runtime bindgen.
- PAM conversation with libc allocation (PAM owns and frees the responses).
- Password in `Zeroizing<String>`, C copy zeroed with volatile writes.
- Service name validation (`[A-Za-z0-9_-]`, no path traversal) and username validation.
- Generic errors: they never reveal whether the user exists.
- UI rate limit with backoff after failed attempts.
- The password is never logged.

**Compositor prerequisite**: some compositors restrict `ext-session-lock-v1` to
privileged clients. If `lard lock` exits with "ext-session-lock unavailable", check
niri's/`ext-session-lock` policy.

---

## Greeter / greetd

A Wayland client **cannot be a greeter on its own**: it needs a compositor to host
it. The correct choice is letting greetd own the privilege boundary (PAM, session,
logind) while keeping the greeter unprivileged.

```
greetd → sway kiosk (layer-shell) → lard greeter → greetd.start_session()
                                                            → niri-session (user)
```

greetd config: [`docs/greetd/config.toml`](docs/greetd/config.toml). A dedicated
unprivileged user is required (the installer does not create it):

```bash
sudo useradd --system --create-home --shell /usr/sbin/nologin greeter
```

```toml
[default_session]
command = "sway --config /etc/lard/greeter-sway.conf"
user = "greeter"
```

Sway is only the host compositor and exits when the greeter exits
([`docs/greetd/sway-greeter.conf`](docs/greetd/sway-greeter.conf)). **Cage is not
suitable**: it does not implement `wlr-layer-shell`.

The greeter speaks greetd's protocol over `GREETD_SOCK` (`len: u32` native-endian
framing + JSON, **not** newline-delimited JSON); if the variable is missing it
refuses to start (unless `--preview`). It is a layer-shell client: a host
compositor with `zwlr_layer_shell_v1` is required (sway, niri; not cage).
`--preview` uses in-process PAM but **does not start** a session: it only tries the
UI.

Alternative: use niri itself as the host compositor with
[`docs/greeter-lard.kdl`](docs/greeter-lard.kdl). It is heavier than sway but avoids
a second kind of compositor.

---

## Performance

Design goals, not slogans:

- **Idle = zero frame callbacks.** No 16 ms timer. The shell wakes up for Wayland
  events, niri IPC, D-Bus notifications, or exact timers (clock on the minute,
  OSD/toast deadlines).
- **Real VSync.** `PresentMode::Fifo` + `wl_surface.frame` before the commit; while
  an animation runs it draws at the refresh rate, when it ends it stops.
- **2 draw calls per surface**: one instanced draw for every SDF shape, one for all
  glyphs. Not one draw per widget.
- **Text**: shaping with cosmic-text only when the string changes; later frames do
  an FNV hash + comparison. The R8 atlas uploads each glyph once.
- **Allocation-free GPU path**: reused instance buffers, a single string arena per
  `Scene`, atlas uploaded once. Views may format a few short strings when the state
  changes or during an animation; there is no per-frame shaping or buffer/target
  allocation. `RenderSurface`s are created only while an overlay is visible and
  destroyed when the animation ends.
- **Services off the UI thread**: niri, audio, battery, network, bluetooth and D-Bus
  notifications run on dedicated threads and talk through `calloop::channel`.
- **RAM**: one shared wgpu device, one 2048² atlas (4 MB), embedded fonts (2.7 MB),
  swapchains only for visible surfaces. No GC, no JS runtime.

To diagnose: `RUST_LOG=lard_wayland=trace` logs presented frames with instance and
draw-call counts.

---

## Known limits (important)

1. **Not tested on real niri in every environment.** Compilation, unit tests,
   config parsing and PAM/wgpu linking are verified; visual output and interaction
   with a compositor must be validated in the field.
2. **Backdrop blur**: only through `ext-background-effect-v1`. The blur region is
   clipped to the panels (bar islands, dock, overlays), never the whole surface. If
   niri does not expose it, there is no desktop blur behind the islands; the
   fallback is alpha/glass.
3. **Multi-output**: lockscreen and greeter cover every output with per-surface
   hit-maps; the greeter gives exclusive focus to the first output and OnDemand to
   the others.
4. **Tray**: no SNI icons.
5. **Services**: network and Bluetooth are native via zbus (event-driven); audio
   still uses `pactl`/`wpctl` and is not a native PipeWire client. Battery and
   brightness read sysfs with slow polls.
6. **Brightness**: the slider writes to `/sys/class/backlight`; if your user lacks
   permissions, install `brightnessctl` or add the user to the `video` group / a
   udev rule.
7. **Fractional scaling**: handled as integer scale; the compositor may look
   slightly soft at 1.25/1.5x.
8. **Emoji/images**: not supported in the atlas; avatars show initials.
9. **Hot reload**: automatic via inotify; per-widget spring parameters stay those
   of the start-up.
10. **`pam_acct_mgmt`**: deliberately not called in the lockscreen (see above).
11. **Greeter `--preview`**: does not start a session; do not use it as a real
    login. Running as a user, PAM can only authenticate the current user (logging
    in other users requires greetd/root). It supports a single password, not
    multiple interactive prompts (OTP, security questions).

---

## Roadmap

1. Validation and profiling on niri 26.x (Vulkan + GL), DPI/scale fixes.
2. Native clients: PipeWire, UPower/logind (event-driven battery and brightness).
3. StatusNotifierItem + DBusMenu + PNG/SVG icons (RGBA atlas).
4. RGBA atlas for emoji and icons; CJK fallback fonts already via fontconfig.
5. Multi-output greeter with an output picker.
6. Night light/gamma and brightness through logind (`SetBrightness`).
7. Springs updatable without a restart and per-module selective reload.
8. Greeter: optional embedded host compositor (Smithay) for a true standalone,
   keeping greetd as the privileged backend.
9. Headless Wayland integration tests (headless compositor + surface smoke test).
10. Native packaging (Arch/AUR, Debian, Nix) beyond `install.sh`, plus theme presets.

---

## License

GPL-3.0-or-later (see [`LICENSE`](LICENSE)).
Fonts: Inter (OFL-1.1) and JetBrains Mono (OFL-1.1), licenses in `assets/fonts/`.
