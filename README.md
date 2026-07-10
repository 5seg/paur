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

## Quick start (local dev — no systemd, no paur user)

For a hands-on look at paur without setting up a system user, a
systemd unit, or a builder image, run it from a temp dir as your
normal user. This is enough to exercise the HTTP API, the Web UI,
and the GPG key flow; **AUR builds still need Docker** and a real
config in `~/.config/paur/`.

```sh
# 1. Build
cargo build --release --workspace
(cd web && npm install && npm run build)

# 2. Pick a sandbox data dir and let paur derive everything else.
#    PAUR_DATA_DIR is honored by `Config::load` — it rewrites
#    repo_dir, work_dir, ccache_dir, gpg_home, logs_dir.
export PAUR_DATA_DIR=$HOME/.local/share/paur-dev
./target/release/paur-cli init    # creates dirs, generates a GPG key

# 3. Run the daemon in one terminal
./target/release/paur-cli serve

# 4. Drive it from another terminal
./target/release/paur-cli add hello
./target/release/paur-cli list
./target/release/paur-cli status hello
# (The build will fail without `docker build -t paur-builder:latest container/`,
#  but the API roundtrip and GPG signing are visible.)

# 5. Or open the Web UI
xdg-open http://127.0.0.1:7300/
```

`deploy/README.md` covers the **production** install — creating a
dedicated `paur` system user, the systemd unit, Caddy in front,
and the Chaotic-style `keyring-build` flow for clients.

## Testing

```sh
cargo test --workspace
```

## License

MIT
