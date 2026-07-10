#!/usr/bin/env bash
#
# Build an AUR package inside the paur-builder container.
#
# Usage:
#   build.sh <pkg-name> <aur-url>      # AUR path
#   build.sh local                      # local PKGBUILD path
#
# Layout (host bind-mounts):
#   /work/src   -- freshly cloned AUR repo (or local PKGBUILD dir in
#                  `local` mode, bind-mounted read-only)
#   /work/out   -- resulting .pkg.tar.* files (moved here on success)
#   /ccache     -- ccache dir (persistent across builds on the host)
#
# The script intentionally uses `set -euo pipefail` to fail fast on any
# error and emit a useful exit code to the host's `paur-builder`.

set -euo pipefail

build_aur() {
    local pkg="$1" url="$2"

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

    move_artifacts "AUR ${pkg}"
}

build_local() {
    # Local PKGBUILD mode. The host bind-mounts a directory containing
    # a single PKGBUILD at /work/src (read-only). We copy it into a
    # writable location because makepkg writes pkg/ and src/ alongside
    # the PKGBUILD.
    if [[ ! -f /work/src/PKGBUILD ]]; then
        echo "==> paur: no PKGBUILD at /work/src" >&2
        exit 2
    fi

    echo "==> paur: building local PKGBUILD"
    rm -rf /work/build
    cp -r /work/src /work/build
    cd /work/build

    makepkg \
        --syncdeps \
        --noconfirm \
        --cleanbuild \
        --skippgpcheck \
        --log \
        --noarchive

    # --noarchive stops makepkg from creating the .pkg.tar.* — we
    # package it ourselves below so we can sign with the daemon's
    # GPG key (makepkg signs with whatever happens to be in the
    # container's keyring, which is *not* what we want for a
    # release artifact).
    shopt -s nullglob
    artifacts=( *.pkg.tar.* )
    if (( ${#artifacts[@]} == 0 )); then
        echo "==> paur: makepkg produced no .pkg.tar.* (build failed?)" >&2
        exit 2
    fi

    mkdir -p /work/out
    mv -f "${artifacts[@]}" /work/out/

    echo "==> paur: built ${#artifacts[@]} artifact(s) for local package"
    ls -l /work/out
}

move_artifacts() {
    local label="$1"
    shopt -s nullglob
    artifacts=( *.pkg.tar.* )
    if (( ${#artifacts[@]} == 0 )); then
        echo "==> paur: makepkg produced no .pkg.tar.* (build failed?)" >&2
        exit 2
    fi

    mkdir -p /work/out
    mv -f "${artifacts[@]}" /work/out/

    echo "==> paur: built ${#artifacts[@]} artifact(s) for ${label}"
    ls -l /work/out
}

# Dispatch. Defined last so all functions are visible.
mode="${1:-}"

case "$mode" in
    local)
        build_local
        ;;
    *)
        pkg="${1:?package name required}"
        url="${2:?aur url required}"
        build_aur "$pkg" "$url"
        ;;
esac
