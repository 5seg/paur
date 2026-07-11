#!/usr/bin/env bash
#
# Build an AUR package inside the paur-builder container.
#
# Usage:
#   build.sh <pkg-name> <aur-url>      # AUR path
#   build.sh local                      # local PKGBUILD path
#   build.sh repo <db.tar.gz> <pkg...>  # repo-add: register the listed
#                                        # .pkg.tar.* files into the DB
#   build.sh unrepo <db.tar.gz> <name>  # repo-remove: drop a package
#
# Layout (host bind-mounts):
#   /work/src   -- freshly cloned AUR repo (or local PKGBUILD dir in
#                  `local` mode, bind-mounted read-only)
#   /work/out   -- resulting .pkg.tar.* files (moved here on success)
#   /work/repo  -- the architecture-specific pacman repo directory
#                  (only mounted for `repo` and `unrepo` modes)
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

    # Apply the per-package RUSTFLAGS override before invoking makepkg.
    # We *append* to whatever the PKGBUILD / makepkg.conf set: rustc
    # honors the last -C codegen-units=N, so this only takes effect
    # if the package didn't already pin a different value.
    if [[ "${PAUR_RUST_CGU:-0}" == "1" ]]; then
        export RUSTFLAGS="${RUSTFLAGS:-} -C codegen-units=1"
        echo "==> paur: RUSTFLAGS=${RUSTFLAGS} (codegen-units=1)"
    fi

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

    if [[ "${PAUR_RUST_CGU:-0}" == "1" ]]; then
        export RUSTFLAGS="${RUSTFLAGS:-} -C codegen-units=1"
        echo "==> paur: RUSTFLAGS=${RUSTFLAGS} (codegen-units=1)"
    fi

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

# Register already-staged .pkg.tar.* files into the repo DB. The host
# bind-mounts the architecture-specific repo dir at /work/repo and the
# signed (or unsigned) .pkg.tar.* files at /work/stage. We do NOT sign
# here — the daemon signs on the host so the GPG private key never
# leaves `/var/lib/paur/.gnupg`.
#
# Args: <db.tar.gz name> <pkg...>
# Example: build.sh repo paur.db.tar.gz /work/stage/openclaude-*.pkg.tar.zst
build_repo() {
    local db_name="$1"; shift
    if [[ -z "${db_name}" || $# -lt 1 ]]; then
        echo "==> paur: usage: build.sh repo <db.tar.gz> <pkg...>" >&2
        exit 2
    fi
    if [[ ! -d /work/repo ]]; then
        echo "==> paur: /work/repo is not mounted" >&2
        exit 2
    fi
    cd /work/repo
    # repo-add expects the .db.tar.gz suffix on its first positional
    # argument; the .db and .files tarballs it creates share the
    # basename. Pre-existing .sig is removed because the DB content
    # changes on every repo-add — the daemon will re-sign afterwards.
    rm -f "${db_name}.sig"
    repo-add "${db_name}" "$@"
    echo "==> paur: repo-add updated ${db_name}"
    ls -l "${db_name}"*
}

# Drop a package from the repo DB. The .pkg.tar.* file itself is left
# in place on the host; the daemon is responsible for unlinking it.
#
# Args: <db.tar.gz name> <pkgname>
build_unrepo() {
    local db_name="$1"; shift
    if [[ -z "${db_name}" || $# -ne 1 ]]; then
        echo "==> paur: usage: build.sh unrepo <db.tar.gz> <pkgname>" >&2
        exit 2
    fi
    if [[ ! -d /work/repo ]]; then
        echo "==> paur: /work/repo is not mounted" >&2
        exit 2
    fi
    cd /work/repo
    rm -f "${db_name}.sig"
    repo-remove "${db_name}" "$1"
    echo "==> paur: repo-remove dropped $1 from ${db_name}"
}

# Dispatch. Defined last so all functions are visible.
mode="${1:-}"

case "$mode" in
    local)
        build_local
        ;;
    repo)
        shift
        build_repo "$@"
        ;;
    unrepo)
        shift
        build_unrepo "$@"
        ;;
    *)
        pkg="${1:?package name required}"
        url="${2:?aur url required}"
        build_aur "$pkg" "$url"
        ;;
esac
