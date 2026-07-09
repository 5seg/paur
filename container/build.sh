#!/usr/bin/env bash
#
# Build an AUR package inside the paur-builder container.
#
# Usage: build.sh <pkg-name> <aur-url>
#
# Layout (host bind-mounts):
#   /work/src   -- freshly cloned AUR repo
#   /work/out   -- resulting .pkg.tar.* files (moved here on success)
#   /ccache     -- ccache dir (persistent across builds on the host)
#
# The script intentionally uses `set -euo pipefail` to fail fast on any
# error and emit a useful exit code to the host's `paur-builder`.

set -euo pipefail

pkg="${1:?package name required}"
url="${2:?aur url required}"

echo "==> paur: building ${pkg} from ${url}"

# Clone fresh on every build. SRCDEST defaults to /work/src so cached
# sources (downloaded by makepkg on a previous build) are reused.
rm -rf /work/src
git clone --quiet "${url}" /work/src
cd /work/src

# --syncdeps: install missing makedepends via pacman.
# --noconfirm: never prompt.
# --cleanbuild: drop pkg/ and src/ before building, so a stale
#               half-built tree can't poison the result.
# --skippgpcheck: many AUR packages have no GPG signature; checking
#                 just produces noise. Re-enable per-package if you
#                 need it.
makepkg \
    --syncdeps \
    --noconfirm \
    --cleanbuild \
    --skippgpcheck \
    --log \
    --holdver

# Move artifacts out. `makepkg` produces exactly one .pkg.tar.* per
# single-package build. The trailing glob picks it up.
shopt -s nullglob
artifacts=( *.pkg.tar.* )
if (( ${#artifacts[@]} == 0 )); then
    echo "==> paur: makepkg produced no .pkg.tar.* (build failed?)" >&2
    exit 2
fi

mkdir -p /work/out
mv -f "${artifacts[@]}" /work/out/

echo "==> paur: built ${#artifacts[@]} artifact(s) for ${pkg}"
ls -l /work/out
