# paur — public install (non-Arch host)

This guide walks through deploying paur on a **non-Arch Linux
server** (Ubuntu, Debian, RHEL, Fedora, etc.) and publishing it
at a public hostname. The example hostname is `paur.5seg.top`;
swap it for your own.

You handle DNS and TLS in front of paur (Cloudflare, your own
reverse proxy, an internal CA — whatever fits). paur's local
Caddy is configured to **not** request certificates; it serves
plain HTTP on `127.0.0.1:80` and lets the upstream proxy handle
TLS termination.

The paur daemon itself is OS-agnostic — it's a single Rust binary
that needs `gpg`, `git`, `repo-add` (from `pacman-contrib`-style
tools), and a container runtime. We build from source on the
target host so we don't ship prebuilt binaries.

## 0. Pick a host

Anything that runs systemd and a recent kernel works. Disk and
CPU are dominated by AUR builds, so plan for ~5 GB free disk and
a few cores. RAM ~2 GB is enough for one concurrent build.

## 1. Install the host dependencies

**Ubuntu / Debian**:
```sh
sudo apt update
sudo apt install -y \
    build-essential pkg-config libssl-dev curl ca-certificates \
    gnupg git systemd-container podman \
    rustup
rustup default stable
```

**RHEL / Fedora**:
```sh
sudo dnf install -y \
    gcc gcc-c++ make pkg-config openssl-devel curl ca-certificates \
    gnupg git systemd \
    podman \
    rustup
rustup default stable
```

`podman` is interchangeable with `docker`; set
`container_runtime = "podman"` in `config.toml` if you prefer it.

## 2. Build paur from source

```sh
git clone https://github.com/<you>/paur /opt/paur-src
cd /opt/paur-src
cargo build --release --workspace
(cd web && npm install && npm run build)
```

Outputs:
- `target/release/paur` — the daemon
- `target/release/paur-cli` — the CLI
- `web/build/` — the static UI bundle

## 3. Install the binaries

```sh
sudo install -m0755 /opt/paur-src/target/release/paur      /usr/local/bin/paur
sudo install -m0755 /opt/paur-src/target/release/paur-cli  /usr/local/bin/paur-cli
```

`/usr/local/bin` keeps them off the package manager; the systemd
unit we install later refers to the same paths.

## 4. Create the system user and data dir

```sh
sudo useradd --system --home /var/lib/paur --shell /usr/sbin/nologin paur
sudo install -d -o paur -g paur /var/lib/paur
```

## 5. Drop the config

```sh
sudo install -m0644 /opt/paur-src/deploy/paur.toml.example \
                   /var/lib/paur/config.toml
sudo chown paur:paur /var/lib/paur/config.toml
sudo $EDITOR /var/lib/paur/config.toml
```

Set at minimum:
- `data_dir = "/var/lib/paur"` (and the rest of the derived
  paths if you move them off the default)
- `public_base_url = "https://paur.5seg.top"` (what clients see
  in their pacman.conf)
- `listen = "127.0.0.1:7300"`
- `container_runtime = "podman"` (or `"docker"`)
- `builder_image = "paur-builder:latest"`

## 6. Build the builder image

paur builds AUR packages inside a container so the host's
toolchain can't pollute the result. Build the image once:

```sh
sudo -u paur podman build -t paur-builder:latest /opt/paur-src/container/
```

(`docker build` if that's your runtime.)

## 7. First-run init

Generates the GPG signing key, creates the per-architecture repo
subdirectory, and exports the public key:

```sh
sudo -u paur paur-cli init
```

The keyid is stored in the `settings` table; the public key lives
at `/var/lib/paur/repo/<arch>/paur.pubkey.asc`.

## 8. Build the keyring + mirrorlist meta-packages

This is what makes client setup a three-line affair:

```sh
sudo -u paur paur-cli keyring-build
```

Two `any`-arch packages are built inside the builder container
and published into the repo:

- `paur-keyring-<ver>-any.pkg.tar.zst`
- `paur-mirrorlist-<ver>-any.pkg.tar.zst`

The exact filenames are printed at the end of the command.
You'll use them as `pacman -U` URLs on each client.

Re-run this command whenever `public_base_url` or the GPG key
changes (after bumping the `pkgver` in the generated PKGBUILDs).

## 9. systemd unit

```sh
sudo install -m0644 /opt/paur-src/deploy/paur.service \
                   /etc/systemd/system/paur.service
sudo systemctl daemon-reload
sudo systemctl enable --now paur
sudo systemctl status paur
```

The unit is distro-agnostic. It sets `GNUPGHOME` and
`PAUR_DATA_DIR` so the daemon runs under the `paur` user without
needing a shell.

## 10. Front it with Caddy (no TLS)

If you're terminating TLS in front of Caddy (Cloudflare, your
own edge, …), use `deploy/Caddyfile.public`. It binds Caddy to
`127.0.0.1:80` only and forwards to the daemon on `:7300`.

```sh
sudo apt install -y caddy      # or: sudo dnf install caddy
sudo install -m0644 /opt/paur-src/deploy/Caddyfile.public \
                   /etc/caddy/Caddyfile
# Edit the hostname in /etc/caddy/Caddyfile
sudo $EDITOR /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

If you'd rather have Caddy fetch and renew its own certificate
(via Let's Encrypt), use the stock `deploy/Caddyfile` and let
port 443 reach the host directly.

## 11. DNS and TLS (your part)

Point `paur.5seg.top` at the host's public IP, then put a TLS
terminator in front of Caddy. The simplest is a Cloudflare
"Full" or "Full (Strict)" record with the Caddy host as the
origin — Cloudflare fetches and serves the cert, Caddy just sees
plain HTTP on `:80`.

Verify from outside:
```sh
curl -I https://paur.5seg.top/
curl -I https://paur.5seg.top/repo/x86_64/paur.db.tar.gz
```

Both should return `200` and not 5xx.

## 12. Add a package

From the build host (or any box that can reach the daemon):

```sh
paur-cli add paru-bin
paur-cli list
paur-cli logs paru-bin --follow
```

Or open `https://paur.5seg.top/` in a browser and use the Web
UI.

## 13. Set up a client

On each Arch machine that should use the repo:

```sh
# 1. Install the keyring and mirrorlist meta-packages.
sudo pacman -U 'https://paur.5seg.top/repo/x86_64/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U 'https://paur.5seg.top/repo/x86_64/paur-mirrorlist-1-1-any.pkg.tar.zst'

# 2. Enable the repo.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist
EOF

# 3. Sync and install.
sudo pacman -Sy hello-pkg
```

No `pacman-key --recv-keys`, no `--lsign-key`, no manual
fingerprint copy-paste. The keyring package places the pubkey at
`/usr/share/pacman/keyrings/paur.asc`; pacman trusts it
automatically once the `[paur]` section is enabled.

## Troubleshooting

- `paur-cli doctor` lists missing host tools (gpg, git,
  repo-add, the container runtime).
- `journalctl -u paur -f` shows the daemon's stderr.
- Build logs are at `/var/lib/paur/logs/<id>.log` and viewable
  in the Web UI.
- A crashed daemon reaps stale `running` rows on next start;
  queued work is preserved.

For more, see `deploy/README.md` (Arch-host install) and
`README.md` (project overview).
