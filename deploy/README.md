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

Three blocks, copy-pasted. Nothing to install beyond what Arch
already ships with — no keyservers, no manual fingerprint lookup.

The only step that touches the network is the two `pacman -U`
commands; they fetch directly from your paur server.

```sh
# 1. Install the keyring and mirrorlist meta-packages.
#    Replace paur.example with your hostname and adjust the
#    <ver> parts to match the files `paur-cli keyring-build`
#    printed for you. (Or use the URLs it printed verbatim.)
sudo pacman -U 'https://paur.example/repo/x86_64/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U 'https://paur.example/repo/x86_64/paur-mirrorlist-1-1-any.pkg.tar.zst'
```

The `paur-keyring` package drops the GPG pubkey at the standard
`/usr/share/pacman/keyrings/paur.asc` path; the `paur-mirrorlist`
package drops the `Server =` line at `/etc/pacman.d/paur-mirrorlist`.
You do **not** need `pacman-key --recv-keys` or `--lsign-key` —
pacman trusts keys in `/usr/share/pacman/keyrings/` automatically
when matching the `Repo =` line in the mirrorlist.

```sh
# 2. Enable the repo by adding one line to pacman.conf.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist
EOF
```

```sh
# 3. Sync and install
sudo pacman -Sy hello-pkg
```

If `pacman -Sy` rejects the signature, the most common cause is
that step 1's `paur-keyring` install didn't actually drop a key at
`/usr/share/pacman/keyrings/paur.asc` — `ls /usr/share/pacman/keyrings/`
should show it. The other common cause is a typo in
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
