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

6. **Build the keyring and mirrorlist meta-packages** (one-time,
   re-run whenever `public_base_url` or the GPG key changes):

   ```sh
   sudo -u paur paur-cli keyring-build
   ```

   This builds two tiny `any`-arch packages inside the builder
   container and publishes them into the repo:

   - `paur-keyring-<ver>-any.pkg.tar.zst` — installs the GPG public
     key to `/usr/share/pacman/keyrings/paur.asc`.
   - `paur-mirrorlist-<ver>-any.pkg.tar.zst` — installs
     `/etc/pacman.d/paur-mirrorlist` containing the `Server =` line
     for your `public_base_url`.

   The exact filenames are printed at the end of the command — they
   are the URLs your clients will `pacman -U` below. (Bump the
   `pkgver` in the generated PKGBUILDs and re-run if you ever need
   to push a key rotation or URL change to clients.)

7. **Install the systemd unit and start the daemon:**

   ```sh
   sudo install -m0644 deploy/paur.service /etc/systemd/system/paur.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now paur
   ```

8. **Front it with Caddy** (or your favourite reverse proxy):

   ```sh
   sudo install -m0644 deploy/Caddyfile /etc/caddy/Caddyfile.d/paur
   sudo systemctl reload caddy
   ```

   If you'd rather serve the static UI and repo with something else,
   the daemon's HTTP API is at `127.0.0.1:7300/api/v1/*` and the repo
   is at `/var/lib/paur/repo/x86_64/`.

9. **Deploy the static UI** into `<data_dir>/webui/`. This is the
   directory `deploy/Caddyfile` serves `/` from. The deployer (you,
   or your CI) is the *only* writer of this directory; the daemon
   never touches it. Keep them separate so a `chown` mistake during
   deploy can't crash the daemon by making `repo/` unwritable.

   ```sh
   sudo install -d -m0755 /var/lib/paur/webui
   (cd web && npm ci && npm run build)
   sudo rsync -a --delete web/build/ /var/lib/paur/webui/
   ```

   Re-run this step whenever you ship UI changes. `--delete` is
   important so stale `_app/` chunks don't keep clients pinned to
   old bundles.

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

Four short blocks, copy-pasted. Nothing to install beyond what
Arch already ships with. The bootstrap step (`pacman-key --add` +
`--lsign-key`) is needed because paur's GPG key is generated
locally on the server and is **not** published to a public
keyserver — so the very first `pacman -U` has no way to validate
the keyring package's signature until you've manually added the
key. After that, every subsequent `pacman -U` / `pacman -Sy` of
a `paur-*` package is signature-validated automatically by the
key the keyring pkg installed. Chaotic-aur's bootstrap works the
same way (just with `pacman-key --recv-keys` instead of `--add`,
since their key is on a keyserver).

```sh
# 0. Find the server's GPG fingerprint. `paur-cli keyring-build`
#    printed it on the server; if you don't have it, read it
#    from the served pubkey:
FPR=$(curl -sSL https://paur.example/repo/x86_64/paur.pubkey.asc \
        | gpg --import-options show-only --import 2>/dev/null \
        | awk '/^pub/{print $2}' | head -1 | cut -d/ -f2)
echo "$FPR"   # 40 hex chars

# 1. Add the pubkey to the local keyring and mark it trusted.
sudo pacman-key --add <(curl -sSL https://paur.example/repo/x86_64/paur.pubkey.asc)
sudo pacman-key --lsign-key "$FPR"

# 2. Install the keyring and mirrorlist meta-packages.
sudo pacman -U 'https://paur.example/repo/x86_64/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U 'https://paur.example/repo/x86_64/paur-mirrorlist-1-1-any.pkg.tar.zst'
```

The `paur-keyring` package's post-install hook runs
`pacman-key --populate paur`, which imports
`/usr/share/pacman/keyrings/paur.gpg` into the local keyring
and `lsign-key`s every FPR listed in
`/usr/share/pacman/keyrings/paur-trusted` at full trust. From
then on, the explicit `--lsign-key` in step 1 is redundant
(the keyring pkg upgrades the trust to level 4) but harmless.
The `paur-mirrorlist` package drops the `Server =` line at
`/etc/pacman.d/paur-mirrorlist`.

```sh
# 3. Enable the repo by adding one line to pacman.conf.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist
EOF
```

```sh
# 4. Sync and install
sudo pacman -Sy hello-pkg
```

If `pacman -Sy` rejects the signature, the most common cause is
that step 1's `pacman-key --lsign-key` was skipped or used the
wrong FPR — `pacman-key --list-sigs paur` should show the
fingerprint with `[full]`. The other common cause is a typo in
`/etc/pacman.d/paur-mirrorlist`; the file is human-readable, fix
it with `sudoedit`.

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
