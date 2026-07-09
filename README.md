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
- `deploy/paur.service` — systemd unit
- `deploy/paur.toml.example` — sample config
- `deploy/README.md` — install walkthrough

## Quick start

```sh
# 1. Build the daemon
cargo build --release --workspace

# 2. Build the UI
cd web && npm install && npm run build && cd ..

# 3. Install (see deploy/README.md for the full flow)
sudo install -m0755 target/release/paur      /usr/bin/paur
sudo install -m0755 target/release/paur-cli  /usr/bin/paur-cli
sudo install -d -o paur -g paur /var/lib/paur
sudo install -m0644 deploy/paur.toml.example /var/lib/paur/config.toml
sudo -u paur paur-cli init
sudo systemctl enable --now paur
```

Then on a client:

```sh
# add to /etc/pacman.conf:
#   [paur]
#   SigLevel = Optional TrustedOnly
#   Server = https://paur.example/$arch
sudo pacman-key --recv-keys <keyid from `paur-cli pubkey`>
sudo pacman-key --lsign-key <keyid>
sudo pacman -Sy <package>
```

## Testing

```sh
cargo test --workspace
```

## License

MIT
