# paur — install

A short field guide for going from a clean Arch host to a working
`pacman -Sy hello-pkg` against your own paur repo.

## One-time setup

1. **Create the system user and directories.**

   ```sh
   sudo useradd --system --home /var/lib/paur --shell /usr/bin/nologin paur
   sudo install -d -o paur -g paur /var/lib/paur
   ```

2. **Install the `paur` binary** (and `paur-cli`).

   ```sh
   sudo install -m0755 target/release/paur      /usr/bin/paur
   sudo install -m0755 target/release/paur-cli  /usr/bin/paur-cli
   ```

3. **Build the builder image** (once, on the host). The image is the
   `archlinux:latest` base with `base-devel`, `git`, `ccache`, and
   `pacman-contrib` pre-installed.

   ```sh
   sudo -u paur docker build -t paur-builder:latest container/
   ```

4. **Drop the config** in place:

   ```sh
   sudo install -m0644 -o paur -g paur deploy/paur.toml.example /var/lib/paur/config.toml
   $EDITOR /var/lib/paur/config.toml   # set public_base_url
   ```

5. **Run `paur-cli init`** (as the `paur` user) to generate the
   signing GPG key, create directories, and export the public key:

   ```sh
   sudo -u paur paur-cli init
   ```

   The keyid is written to the `settings` table; the public key is
   exported to `/var/lib/paur/repo/x86_64/paur.pubkey.asc`.

6. **Install the systemd unit and start the daemon:**

   ```sh
   sudo install -m0644 deploy/paur.service /etc/systemd/system/paur.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now paur
   ```

7. **Front it with Caddy** (or your favourite reverse proxy):

   ```sh
   sudo install -m0644 deploy/Caddyfile /etc/caddy/Caddyfile.d/paur
   sudo systemctl reload caddy
   ```

   If you'd rather serve the static UI and repo with something else,
   the daemon's HTTP API is at `127.0.0.1:7300/api/v1/*` and the repo
   is at `/var/lib/paur/repo/x86_64/`.

## Adding packages

From any host that can reach the daemon:

```sh
paur-cli add paru-bin
paur-cli list
paur-cli logs paru-bin --follow
paur-cli remove paru-bin
```

Or use the web UI at `https://paur.example/`.

## On the client machine

```sh
# 1. Add the repo
sudo tee -a /etc/pacman.conf <<'EOF'
[paur]
SigLevel = Optional TrustedOnly
Server = https://paur.example/$arch
EOF

# 2. Trust the signing key. The keyid is the long fingerprint
#    shown by `paur-cli pubkey` (or visible in the web UI's
#    /pubkey page).
sudo pacman-key --recv-keys <keyid>
sudo pacman-key --lsign-key <keyid>

# 3. Sync and install
sudo pacman -Sy hello-pkg
```

## Auto-rebuild

Mark a package for automatic rebuild on AUR HEAD change:

```sh
paur-cli add paru-bin --auto-rebuild
```

The daemon polls `git ls-remote` every `poll_interval_secs` (default
600s) and enqueues a new build when the ref advances.

## Logs and troubleshooting

- Build logs are persisted to the DB and to `/var/lib/paur/logs/`.
- `paur-cli doctor` checks that `docker`, `podman`, `repo-add`,
  `gpg`, `git`, and `makepkg` are on PATH.
- `journalctl -u paur` shows the daemon's stderr.
- Stale `running` rows from a crashed daemon are reaped on the next
  start; queued work is preserved.
