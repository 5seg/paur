#!/usr/bin/env bash
#
# Build an AUR package inside the paur-builder container.
#
# Usage:
#   build.sh <pkg-name> <aur-url>        # AUR path
#   build.sh local                        # local PKGBUILD path
#   build.sh repo <db.tar.gz> <arch-dir> <pkg...>   # repo-add for a variant
#   build.sh unrepo <db.tar.gz> <arch-dir> <name>   # repo-remove
#
# Layout (host bind-mounts):
#   /work/src   -- freshly cloned AUR repo (or local PKGBUILD dir in
#                  `local` mode, bind-mounted read-only)
#   /work/out   -- resulting .pkg.tar.* files (moved here on success)
#   /work/repo  -- the architecture-specific pacman repo directory
#                  for one variant (default, v3, or v4). The host
#                  passes the absolute path as the second positional
#                  arg to `repo` / `unrepo` so the same image can
#                  target any of the three arch subdirs.
#   /ccache     -- ccache dir (persistent across builds on the host)
#
# The script intentionally uses `set -euo pipefail

# Apply a per-package x86-64 microarchitecture level when the
# daemon hands us PAUR_MARCH=v3 or PAUR_MARCH=v4. The recipe is
# CachyOS-style:
#   CFLAGS="-march=x86-64-vN -O2 -pipe -fno-plt"
#   CXXFLAGS=${CFLAGS}
#   RUSTFLAGS=${RUSTFLAGS:-} -C target-cpu=x86-64-vN
# - CFLAGS is replaced wholesale (the container's default
#   makepkg.conf uses generic x86-64; we override that for
#   the whole build).
# - CXXFLAGS follows CFLAGS.
# - RUSTFLAGS is *appended* so PKGBUILDs that export their own
#   RUSTFLAGS (LTO, codegen-units, etc.) are preserved.
# When PAUR_MARCH is unset or empty, this is a no-op and the
# container's default flags take over.
apply_march() {
    case "${PAUR_MARCH:-}" in
        v3|v4)
            export CFLAGS="-march=x86-64-${PAUR_MARCH} -O2 -pipe -fno-plt"
            export CXXFLAGS="${CFLAGS}"
            export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=x86-64-${PAUR_MARCH}"
            echo "==> paur: march=${PAUR_MARCH} CFLAGS=${CFLAGS}"
            ;;
        "")
            : # no-op: use container makepkg.conf defaults
            ;;
        *)
            echo "==> paur: ignoring unknown PAUR_MARCH='${PAUR_MARCH}'" >&2
            ;;
    esac
}

set -euo pipefail

build_aur() {
    local pkg="$1" url="$2"

    echo "==> paur: building ${pkg} from ${url}"

    # Clone fresh on every build. SRCDEST defaults to /work/src so cached
    # sources (downloaded by makepkg on a previous build) are reused.
    rm -rf /work/src
    git clone --quiet "${url}" /work/src
    cd /work/src

    # Apply the per-package x86-64 march level (no-op if PAUR_MARCH
    # is unset). Done after the cd so the function's CFLAGS /
    # CXXFLAGS / RUSTFLAGS exports only affect this build.
    apply_march

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
    # /work/build is a fresh tmpfs (or scratch dir) supplied by the
    # host on each invocation, so it never has leftover content to
    # clean up. Trying to `rm -rf` it from inside the container
    # would fail with "Device or resource busy" because it's a
    # mount point.
    #
    # The trailing `/.` is important: `cp -r /work/src /work/build`
    # would copy the contents into `/work/build/src/` (because
    # /work/build is itself a directory) and makepkg would then
    # look for `/work/build/PKGBUILD` and not find it. Copying the
    # *contents* of /work/src straight into /work/build gives us
    # `/work/build/PKGBUILD` and `/work/build/paur.pubkey.asc`.
    cp -r /work/src/. /work/build/
    cd /work/build

    if [[ "${PAUR_RUST_CGU:-0}" == "1" ]]; then
        export RUSTFLAGS="${RUSTFLAGS:-} -C codegen-units=1"
        echo "==> paur: RUSTFLAGS=${RUSTFLAGS} (codegen-units=1)"
    fi

    # Apply the per-package x86-64 march level (no-op if PAUR_MARCH
    # is unset).
    apply_march

    # Force PKGDEST to the current directory (where the PKGBUILD
    # was copied to). makepkg's default of `$startdir` ought to
    # match, but the bind-mounted /work dir interacts oddly with
    # the container's WORKDIR (which we no longer set in the
    # Dockerfile but some user images may still have), so we set
    # PKGDEST explicitly.
    PKGDEST="$PWD" makepkg \
        --syncdeps \
        --noconfirm \
        --cleanbuild \
        --skippgpcheck \
        --log \
        --nosign

    # --nosign stops makepkg from creating a pacman GPG signature
    # (the container's keyring either doesn't exist or has the wrong
    # key). The daemon signs the artifact with the host keyring in
    # `paur_repo::publish` after repo-add; this is the same path
    # regular AUR builds take. We do *not* use `--noarchive` because
    # the existing `move_artifacts` logic expects the .pkg.tar.* on
    # disk.
    shopt -s nullglob
    # Look for .pkg.tar.* first in /work/build (the expected place),
    # then fall back to the entire /work tree in case PKGDEST got
    # overridden by something downstream of the script.
    artifacts=( *.pkg.tar.* )
    if (( ${#artifacts[@]} == 0 )); then
        artifacts=( /work/*.pkg.tar.* /work/*/*.pkg.tar.* )
    fi
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
    local arch_dir="$1"; shift
    if [[ -z "${db_name}" || -z "${arch_dir}" || $# -lt 1 ]]; then
        echo "==> paur: usage: build.sh repo <db.tar.gz> <arch_dir> <pkg...>" >&2
        exit 2
    fi
    if [[ ! -d "${arch_dir}" ]]; then
        echo "==> paur: arch_dir ${arch_dir} is not a directory" >&2
        exit 2
    fi
    cd "${arch_dir}"
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
    local arch_dir="$1"; shift
    if [[ -z "${db_name}" || -z "${arch_dir}" || $# -ne 1 ]]; then
        echo "==> paur: usage: build.sh unrepo <db.tar.gz> <arch_dir> <pkgname>" >&2
        exit 2
    fi
    if [[ ! -d "${arch_dir}" ]]; then
        echo "==> paur: arch_dir ${arch_dir} is not a directory" >&2
        exit 2
    fi
    cd "${arch_dir}"
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
