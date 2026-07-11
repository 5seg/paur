# paur build container
#
# A throwaway Arch environment for building AUR packages with makepkg.
# It is intentionally minimal: base-devel + the pacman/contrib tools we
# need to compute checksums. ccache is included for the common case
# where builds are repeated.

FROM archlinux:latest

# Keep package installs reproducible: refresh keys + upgrade.
RUN pacman-key --init \
 && pacman-key --populate archlinux \
 && pacman -Syu --noconfirm \
      base-devel \
      git \
      ccache \
      pacman-contrib \
      openssh \
 && pacman -Scc --noconfirm

# Build as a non-root user inside the container. The user does not
# need a password because the container is already isolated; the
# password requirement of `makepkg` is only to discourage running as
# root in *unisolated* environments.
RUN useradd -m -G wheel builder \
 && echo "builder ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers

# A writable mount points for the build inputs/outputs (host bind-mounts
# these). ccache is bind-mounted separately so its contents survive
# across builds.
VOLUME ["/work", "/ccache"]
# Intentionally do NOT set WORKDIR here. The bind-mounted /work dir
# is owned by the host's paur user (mode 0o777) but with WORKDIR=/work
# some makepkg invocations resolve PKGDEST relative to /work
# regardless of the script's `cd` step, dumping .pkg.tar.* into
# the bind mount root rather than the per-build subdir. Setting
# cwd explicitly inside build.sh (`cd /work/build` etc.) is
# more reliable.

# Entry point lives at a stable path so the host can invoke it
# directly via `docker run ... paur-builder /build.sh <pkg> <aur_url>`.
COPY build.sh /build.sh
RUN chmod +x /build.sh

USER builder
ENV CCACHE_DIR=/ccache \
    CCACHE_MAXSIZE=2G \
    MAKEFLAGS="-j$(nproc)"

ENTRYPOINT ["/build.sh"]
