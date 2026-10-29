# Updating LARD

LARD has two update channels. Both end up restarting the running bar with the
new binary.

## 1. `lard update`

```bash
lard update              # update with the official installer
lard update --source     # force a source rebuild
lard update --dry-run    # show what the updater would do
```

`lard update` behaves differently depending on how LARD was installed:

- **Package-managed** (`pacman -Qo`/`dpkg -S`/`rpm -qf` knows the binary): it
  asks the system package manager to upgrade the `lard` package, so
  `pacman -Syu` and `lard update` stay consistent.
- **Script install** (`/usr/local/bin/lard`): it downloads the official
  bootstrap from <https://lard.francescocallegaro.com/install.sh> and runs it
  with `--update`, which:
  - keeps `/etc/lard/config.toml`, the PAM service and the boot choice;
  - replaces the managed niri/kitty/systemd files (backing up only what
    actually changed);
  - reinstalls the binary and restarts `lard.service`.

The updater never changes the boot setup: `--update` reads the current one
(greetd, tty1 autologin, or none) and keeps it.

## 2. System package manager

Install the package once and the normal system upgrade updates LARD too.

From the LARD repository (Arch):

```ini
# /etc/pacman.conf
[lard]
SigLevel = Optional TrustAll
Server = https://lard.francescocallegaro.com/repo/arch/$arch
```

```bash
sudo pacman -Syu lard
```

Or build it yourself / from the AUR (`lard-git`):

```bash
git clone https://github.com/francescokalle/lard
cd lard/packaging/arch
makepkg -si          # package `lard`
# or: cd lard-git && makepkg -si
```

Once the package owns `/usr/bin/lard`, `pacman -Q lard` reports it and
`lard update` delegates to pacman instead of rebuilding from source.

## Publishing a release (maintainers)

```bash
./scripts/publish-arch-repo.sh          # builds packaging/arch/PKGBUILD
```

The script tars the working tree, builds the package on the machine running it,
writes it to `site/repo/arch/x86_64/` and refreshes `lard.db.tar.gz` with
`repo-add`. The site is served from that directory, so the new version is
available to `pacman -Syu` immediately.
