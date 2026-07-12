<script lang="ts">
  // Install page. A single copy-pasteable bootstrap for a fresh
  // Arch client. The host is auto-filled from `location.host` so
  // the rendered script "just works" for almost every deployment;
  // edit it if the client is on a different network.
  //
  // The four logical steps (find FPR, bootstrap key, install
  // keyring + mirrorlist, enable repo) are kept inline as a single
  // bash script rather than split into per-step blocks — chaotic's
  // docs do the same.

  import { onMount } from 'svelte';

  let host = $state('');
  let arch = $state('x86_64');
  let fpr = $state<string | null>(null);
  let fprError = $state<string | null>(null);
  let copied = $state(false);

  onMount(() => {
    host = location.host;
  });

  // Server-side FPR helper. Avoids shipping a PGP parser in the UI
  // bundle: the daemon already has GPG on PATH (it signs repo DBs
  // with it), so we just ask it to parse its own pubkey. The
  // script below does the same parse client-side, so this button
  // is purely a convenience for users who'd rather read the FPR
  // than copy block 1.
  async function fetchFpr() {
    try {
      const r = await fetch(`/api/v1/install/fpr?host=${encodeURIComponent(host)}`);
      if (!r.ok) {
        fprError = `fpr endpoint returned ${r.status}; the script below extracts it client-side instead`;
        return;
      }
      const j = (await r.json()) as { fpr: string };
      fpr = j.fpr;
    } catch (e) {
      fprError = e instanceof Error ? e.message : String(e);
    }
  }

  // Three-repo pacman.conf block. Each repo corresponds to one
  // physical arch dir on the server (`x86_64`, `x86_64-v3`,
  // `x86_64-v4`). v3 / v4 builds are opt-in per package; the repo
  // itself is always present but only carries packages whose
  // variants include that march level. Users who don't want to
  // pull v3 / v4 binaries can comment out those two sections and
  // keep `[paur]` only.
  const baseUrl = $derived(`${location.protocol}//${host}/repo`);
  const script = $derived(
    `# Bootstrap an Arch client against this paur server.
# paur serves THREE pacman repos: default (x86_64) plus opt-in
# x86_64-v3 (Haswell+) and x86_64-v4 (Zen4 / Sapphire Rapids+).
# A package built for a given march is only published to that
# repo; pacman picks the highest-priority enabled repo that has
# the package, so v3 / v4 only "win" when their section is
# enabled AND the package is published there.

# 1. Find this server's GPG fingerprint. The pubkey is identical
#    across all three repos (single key, served once per arch
#    dir). We pull it from the default repo's arch dir.
#
#    We use \`gpg --import\` + \`--list-keys --with-colons\` to
#    extract the full 40-char fingerprint. The lighter
#    \`--import-options show-only\` route is unreliable for
#    ed25519 keys (the \`pub\` line has no Key ID to extract).
#
#    On a fresh install, run \`pacman-key --init\` once before
#    \`--lsign-key\`, otherwise the latter errors with "There is
#    no secret key available to sign with".
#
#    Note: we deliberately avoid bash process substitution
#    (\`<(...)\`) together with \`sudo\`. sudo's default
#    \`closefrom\` closes inherited file descriptors, so the
#    inner fd (\`/dev/fd/NN\`) is gone by the time \`pacman-key\`
#    runs, and it errors with "can't open '/dev/fd/NN': No such
#    file or directory". Writing to a tempfile and passing the
#    path is portable.
PUB=$(mktemp)
trap 'rm -f "$PUB"' EXIT
curl -sSL ${baseUrl}/${arch}/paur.pubkey.asc -o "$PUB"
FPR=$(gpg --import "$PUB" 2>/dev/null \\
        && gpg --list-keys --with-colons 2>/dev/null \\
        | awk -F: '/^fpr/{print $10; exit}')
echo "FPR=$FPR"
sudo pacman-key --init

# 2. Bootstrap: add the pubkey to the local keyring and lsign it.
#    --lsign-key prompts for the local keyring's passphrase,
#    which defaults to empty on a fresh install. The \`sudo cp\`
#    dance below puts the key in a root-owned path so \`pacman-key
#    --add\` can read it.
sudo cp "$PUB" /etc/pacman.d/paur.pubkey.asc
sudo pacman-key --add /etc/pacman.d/paur.pubkey.asc
sudo pacman-key --lsign-key "$FPR"
sudo rm -f /etc/pacman.d/paur.pubkey.asc

# 3. Install the keyring meta-package. The post-install hook on
#    \`paur-keyring\` runs \`pacman-key --populate paur\`, which
#    re-imports the pubkey and lsigns every FPR in \`paur-trusted\`
#    at full trust. (The meta-package lives in the default repo;
#    once installed, it works for the v3 / v4 repos too because
#    the same key signs all three.)
sudo pacman -U --noconfirm '${baseUrl}/${arch}/paur-keyring-1-1-any.pkg.tar.zst'

# 4. Enable the three repos in pacman.conf. Edit the file
#    interactively or paste the block at the end. Each section
#    uses \`\$arch\` so the same block works on x86_64 and any
#    other arch paur publishes for. Comment out [paur-v3] and
#    [paur-v4] if you only want the default builds.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
SigLevel = Required DatabaseOptional
Server = ${baseUrl}/$arch

[paur-v3]
SigLevel = Required DatabaseOptional
Server = ${baseUrl}/$arch-v3

[paur-v4]
SigLevel = Required DatabaseOptional
Server = ${baseUrl}/$arch-v4
EOF
sudo pacman -Sy hello

# Optional: list which packages have a v3 / v4 build available
# without installing them. The variants field is per-package on
# the daemon; the queue page shows what's currently in flight.
# curl -s ${baseUrl}/../api/v1/packages | jq '.[] | {name, variants}'`
  );

  async function copy() {
    try {
      await navigator.clipboard.writeText(script);
      copied = true;
      setTimeout(() => {
        copied = false;
      }, 1500);
    } catch {
      // Best-effort: if clipboard is denied (insecure context),
      // the user can still select-and-copy the rendered text.
    }
  }
</script>

<h1 class="mb-2 text-2xl font-semibold tracking-tight" style="color: var(--ink);">Install</h1>

<p class="mb-6 text-sm" style="color: var(--body);">
  Bootstrap an Arch client against this paur server. The host is
  prefilled from the URL you reached this page on; edit it if the
  client is on a different network.
</p>

<div class="mb-6 flex flex-wrap items-end gap-3">
  <label class="block">
    <span class="mb-1 block text-xs font-medium" style="color: var(--mute);">Server host</span>
    <input
      class="mt-1 block w-64 rounded-md border px-2 py-1.5 text-sm"
      style="background: var(--bg-page); border-color: var(--hairline); color: var(--ink);"
      type="text"
      bind:value={host}
      placeholder="paur.example"
    />
  </label>
  <label class="block">
    <span class="mb-1 block text-xs font-medium" style="color: var(--mute);">Arch</span>
    <input
      class="mt-1 block w-32 rounded-md border px-2 py-1.5 text-sm"
      style="background: var(--bg-page); border-color: var(--hairline); color: var(--ink);"
      type="text"
      bind:value={arch}
      placeholder="x86_64"
    />
  </label>
  <button class="btn" onclick={fetchFpr} type="button">
    Resolve FPR server-side
  </button>
  {#if fpr}
    <div class="rounded-md border px-3 py-2 text-xs" style="border-color: var(--hairline); color: var(--ink);">
      FPR: <code style="color: var(--accent);">{fpr}</code>
    </div>
  {:else if fprError}
    <div class="text-xs" style="color: var(--error);">
      {fprError}
    </div>
  {/if}
</div>

<div class="rounded-md border p-3" style="background: var(--bg-card); border-color: var(--hairline);">
  <div class="mb-2 flex justify-end">
    <button class="btn" type="button" onclick={copy}>
      {copied ? 'Copied' : 'Copy'}
    </button>
  </div>
  <pre class="log-view overflow-x-auto whitespace-pre text-xs" style="color: var(--body);">{script}</pre>
</div>

<p class="mt-4 text-xs" style="color: var(--mute);">
  paur's GPG key is generated locally on the server and is not
  published to any keyserver, so the first <code>pacman -U</code>
  can't validate the keyring package's signature until step 2
  has run. After that, the keyring's post-install hook re-imports
  the pubkey at full trust, so subsequent <code>pacman -U</code>
  and <code>pacman -Sy</code> on <code>paur-*</code> packages are
  signature-validated automatically.
</p>
