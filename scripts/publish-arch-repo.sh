#!/usr/bin/env bash
# Build the Arch package from the current tree and refresh the pacman repository
# served at https://lard.francescocallegaro.com/repo/arch/x86_64.
#
# Run on a build machine with base-devel + rust:
#   ./scripts/publish-arch-repo.sh
#   LARD_PKGVER=0.1.0.r1234 ./scripts/publish-arch-repo.sh
#
# The repository directory is the static site's docroot, so a publish is live
# immediately; commit site/repo/arch/x86_64/lard.db.tar.gz so the GitHub mirror
# serves the same index.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -n "${LARD_PKGVER:-}" ]; then
    VER="$LARD_PKGVER"
elif git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    VER="0.1.0.r$(git -C "$ROOT" rev-list --count HEAD)"
else
    VER="0.1.0.r$(date +%s)"
fi
OUT="${LARD_REPO_OUT:-$ROOT/site/repo/arch/x86_64}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> source lard-$VER"
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "$ROOT" archive --prefix="lard-$VER/" -o "$WORK/lard-$VER.tar.gz" HEAD
else
    # Synced tree without .git (the reference box): tar the working copy.
    mkdir -p "$WORK/lard-$VER"
    tar -C "$ROOT" \
        --exclude=./target --exclude=./.git --exclude=./site/repo \
        --exclude=./preview --exclude=./vd-small.png \
        -cf - . | tar -C "$WORK/lard-$VER" -xf -
    tar -C "$WORK" -czf "$WORK/lard-$VER.tar.gz" "lard-$VER"
fi

echo "==> build lard-$VER"
cp "$ROOT/packaging/arch/PKGBUILD" "$WORK/PKGBUILD"
sed -i "s/^pkgver=.*/pkgver=$VER/" "$WORK/PKGBUILD"
(cd "$WORK" && makepkg --force --nodeps --noconfirm)

echo "==> publish to $OUT"
mkdir -p "$OUT"
cp "$WORK"/lard-"$VER"-*.pkg.tar.zst "$OUT/"
(cd "$OUT" && repo-add --remove lard.db.tar.gz lard-"$VER"-1-*.pkg.tar.zst)

cat <<EOF

Repository ready:
  [lard]
  SigLevel = Optional TrustAll
  Server = https://lard.francescocallegaro.com/repo/arch/\$arch
EOF
