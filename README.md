# paur - Pre-build AUR packages for you

A self-hosted, personal AUR pre-build service for Arch Linux.

paur lets you maintain a curated list of AUR packages, builds them in
isolated containers, signs the resulting `.pkg.tar.zst` files with GPG,
and serves them as a pacman-compatible repository over HTTPS. Your
client machines then install the prebuilt packages with one line in
`/etc/pacman.conf` — no `makepkg` on the client.

## Crates

- `paur-core` — config, error, `PkgName` validation, paths
- `paur-db` — SQLite schema, migrations, CRUD
- `paur-aur` — AUR git operations (clone, `ls-remote`, `.SRCINFO`)
- `paur-builder` — `docker run`/`podman run` invocation + log streaming
- `paur-repo` — `repo-add`, `repo-remove`, GPG sign
- `paur-daemon` — queue worker, axum HTTP API, AUR poller (binary: `paur`)
- `paur-cli` — clap front-end (binary: `paur-cli`)

## Web UI

A SvelteKit 2 + Svelte 5 + Tailwind dashboard in `web/`. Talks to the
daemon's `/api/v1` over the same origin (Vite dev server proxies,
Caddy reverse-proxies in production). `npm run build` produces a
static bundle under `web/build/`.

## Deploy

- `container/Dockerfile` — builder image (`paur-builder:latest`)
- `deploy/Caddyfile` — reverse proxy + static repo + static UI
  (Caddy fetches its own Let's Encrypt cert)
- `deploy/Caddyfile.public` — same routing, **no TLS**; use when
  you terminate TLS in front (Cloudflare, an internal proxy, etc.)
- `deploy/paur.service` — systemd unit
- `deploy/paur.toml.example` — sample config
- `deploy/README.md` — install walkthrough (Arch host)
- `deploy/README.linux.md` — install walkthrough (Ubuntu / Debian /
  RHEL / Fedora, with a public hostname like `paur.5seg.top`)

## Quick start

Two roles, one README. Pick the one you want to do.

### 1. Build host (the box that runs paur)

For a hands-on look at paur without setting up a system user,
a systemd unit, or a builder image, run it from a temp dir as
your normal user. This is enough to exercise the HTTP API, the
Web UI, and the GPG key flow; **AUR builds still need Docker**
and `docker build -t paur-builder:latest container/`.

```sh
# 1. Build
cargo build --release --workspace
(cd web && npm install && npm run build)

# 2. Pick a sandbox data dir. PAUR_DATA_DIR is honored by
#    `Config::load` and rewrites repo_dir, work_dir, ccache_dir,
#    gpg_home, logs_dir.
export PAUR_DATA_DIR=$HOME/.local/share/paur-dev
./target/release/paur-cli init    # creates dirs, generates a GPG key

# 3. Run the daemon in one terminal
./target/release/paur-cli serve

# 4. Drive it from another terminal
./target/release/paur-cli add hello
./target/release/paur-cli add gpu-pkg --variant v3 --variant v4
./target/release/paur-cli flag hello --variant v3
./target/release/paur-cli list
./target/release/paur-cli status hello
# (The build fails without `docker build -t paur-builder:latest container/`,
#  but the API roundtrip, variant enqueue, and GPG signing are visible.)
# Each package can opt into extra build variants with
# `paur-cli flag <pkg> --variant v3` (toggle). The default
# variant is always built; v3 and v4 are CachyOS-style
# `-march=x86-64-vN` builds that land in `repo/x86_64-v3/` and
# `repo/x86_64-v4/` respectively. The three arch dirs are
# served side by side so a single `Include` in pacman.conf
# pulls any of them.

# 5. Or open the Web UI
xdg-open http://127.0.0.1:7300/
```

For a **production** install (systemd unit, `paur` system user,
Caddy in front, exposing the repo to other machines), see
`deploy/README.md`.

### 2. Client (the box that does `pacman -Sy <pkg>`)

Replace `paur.example` with the hostname your paur server is
reachable at. The server has already run `paur-cli keyring-build`
once (see `deploy/README.md`) to publish the two meta-packages and
the server's GPG public key into `repo/x86_64/`.

paur's GPG key is generated locally on the server and **not**
published to any public keyserver, so the first `pacman -U` cannot
validate the keyring package's signature out of the box. We work
around this by adding the pubkey to the local keyring from the
server's URL first (chaotic-aur's bootstrap works the same way,
just with a keyserver instead of an HTTP URL).

The FPR is printed on the server when you run `paur-cli init`
(look for the line `generated new key: <FPR>`) and again at the
end of `paur-cli keyring-build`. If you don't have it, fetch the
pubkey once and read the FPR from it directly.

```sh
# 0. (once per client) Find the server's GPG fingerprint.
#    `paur-cli keyring-build` printed it on the server; if you
#    don't have it, read it from the served pubkey:
FPR=$(curl -sSL http://paur.example/repo/x86_64/paur.pubkey.asc \
        | gpg --import-options show-only --import 2>/dev/null \
        | awk '/^pub/{print $2}' | head -1 | cut -d/ -f2)
echo "$FPR"   # 40 hex chars

# 1. Add the pubkey to the local keyring and mark it trusted.
#    This is the one-time bootstrap: we can't use `pacman -U` yet
#    because that would need the key to already be trusted in
#    order to validate the keyring package's own signature.
sudo pacman-key --add <(curl -sSL http://paur.example/repo/x86_64/paur.pubkey.asc)
sudo pacman-key --lsign-key "$FPR"

# 2. Install the keyring + mirrorlist meta-packages. The
#    `paur-keyring` post-install hook runs
#    `pacman-key --populate paur` which imports
#    `/usr/share/pacman/keyrings/paur.gpg` and lsigns every FPR
#    in `/usr/share/pacman/keyrings/paur-trusted` at full trust
#    (so even if step 1 used a different trust level, the
#    keyring pkg upgrades it to full). From this point on every
#    subsequent `pacman -U` / `pacman -Sy` of `paur-*` packages
#    is signature-validated automatically.
sudo pacman -U --noconfirm 'http://paur.example/repo/x86_64/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U --noconfirm 'http://paur.example/repo/x86_64/paur-mirrorlist-1-1-any.pkg.tar.zst'

# 3. Add the repo to pacman.conf. paur publishes to three arch
#    variants (x86_64 / x86_64-v3 / x86_64-v4). A single
#    `[paur]` section pulls all three via the `paur-mirrorlist`
#    Include — pacman matches the right Server line by `$arch`
#    expansion. The mirrorlist ships three `Server =` lines:
#
#      Server = http://paur.example/repo/$arch
#      Server = http://paur.example/repo/$arch-v3
#      Server = http://paur.example/repo/$arch-v4
#
#    `default` (x86_64) is always published. `v3` and `v4` only
#    land in the repo for packages where you've enabled the
#    variant via `paur-cli flag <pkg> --variant v3` (or via
#    the web UI). All three repos share the same GPG key
#    (`paur-keyring` covers them all), so you don't need
#    separate keyring installs.
#
#    The optional `[paur-v3]` / `[paur-v4]` sections below
#    are commented out; uncomment them if you want to control
#    which arch variants pacman can pick. The default section
#    alone is enough to fetch any package — pacman ignores
#    Server lines that point at a non-existent arch dir.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist

#[paur-v3]
#Include = /etc/pacman.d/paur-mirrorlist

#[paur-v4]
#Include = /etc/pacman.d/paur-mirrorlist
EOF

# 4. Sync and install. `hello-pkg` (default only) installs
#    straight from `[paur]`. If you enabled v3 / v4 for the
#    package, pacman pulls the matching `.pkg.tar.zst` from
#    the v3 / v4 arch dir when those sections are enabled.
sudo pacman -Sy hello-pkg
```

If you ever want to publish the key to a keyserver, run
`gpg --send-keys <FPR>` on the server and from then on you can
also `pacman-key --recv-keys <FPR>` on a fresh client instead
of going through `curl` + `pacman-key --add`.

## Testing

```sh
cargo test --workspace
```

## License

MIT
