<script lang="ts">
  // Install page. Renders four copy-pasteable blocks for a fresh
  // Arch client: (0) find the FPR, (1) bootstrap the GPG key,
  // (2) install the keyring + mirrorlist meta-packages, (3) enable
  // the repo in pacman.conf. The hostname defaults to the browser's
  // current host, so the rendered commands "just work" for almost
  // every deployment.
  //
  // The shape mirrors chaotic-aur's docs (https://aur.chaotic.cx/),
  // but we extract the FPR on the client side via the same
  // `gpg --import-options show-only --import` flow rather than
  // relying on a public keyserver.

  import { onMount } from 'svelte';

  let host = $state('');
  let arch = $state('x86_64');
  let fpr = $state<string | null>(null);
  let fprError = $state<string | null>(null);
  let copiedBlock = $state<number | null>(null);

  onMount(() => {
    host = location.host;
  });

  // Server-side FPR helper. Avoids shipping a PGP parser in the UI
  // bundle: the daemon already has GPG on PATH (it signs repo DBs
  // with it), so we just ask it to parse its own pubkey. If the
  // daemon is older than this endpoint, we fall back to the
  // client-side FPR extraction in block 0.
  async function fetchFpr() {
    try {
      const r = await fetch(`/api/v1/install/fpr?host=${encodeURIComponent(host)}`);
      if (!r.ok) {
        fprError = `fpr endpoint returned ${r.status}; copy block 0 verbatim instead`;
        return;
      }
      const j = (await r.json()) as { fpr: string };
      fpr = j.fpr;
    } catch (e) {
      fprError = e instanceof Error ? e.message : String(e);
    }
  }

  const repoRoot = $derived(`${location.protocol}//${host}/repo/${arch}`);

  function block(n: number, title: string, cmd: string) {
    return { n, title, cmd };
  }

  const blocks = $derived([
    // 0. Find the FPR. We can do this client-side via the same
    //    `gpg --import-options show-only --import` the README
    //    documents, OR we can have the daemon do it for us (the
    //    "Resolve FPR server-side" button above). We default to
    //    the client-side flow so the copy-paste works without a
    //    click first.
    block(
      0,
      'Find the FPR',
      `# Find this server's GPG fingerprint (40 hex chars)
FPR=$(curl -sSL ${repoRoot}/paur.pubkey.asc \\
        | gpg --import-options show-only --import 2>/dev/null \\
        | awk '/^pub/{print $2}' | head -1 | cut -d/ -f2)
echo "$FPR"`
    ),
    // 1. Bootstrap: add the pubkey to the local keyring and lsign
    //    it. pacman-key --lsign-key prompts for the local keyring's
    //    passphrase, which defaults to empty on a fresh install.
    block(
      1,
      'Bootstrap GPG key',
      `# Bootstrap: add the pubkey to the local keyring and lsign it.
sudo pacman-key --add <(curl -sSL ${repoRoot}/paur.pubkey.asc)
sudo pacman-key --lsign-key "$FPR"`
    ),
    // 2. Install the keyring + mirrorlist meta-packages. The
    //    post-install hook on `paur-keyring` runs
    //    `pacman-key --populate paur`, which re-imports the
    //    pubkey and lsigns every FPR in `paur-trusted` at full
    //    trust.
    block(
      2,
      'Install keyring + mirrorlist',
      `# Install the keyring + mirrorlist meta-packages.
sudo pacman -U --noconfirm '${repoRoot}/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U --noconfirm '${repoRoot}/paur-mirrorlist-1-1-any.pkg.tar.zst'`
    ),
    // 3. Enable the repo + first sync.
    block(
      3,
      'Enable in pacman.conf',
      `# Enable the repo in pacman.conf.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist
EOF

# Sync and install.
sudo pacman -Sy hello-pkg`
    )
  ]);

  async function copy(n: number, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copiedBlock = n;
      setTimeout(() => {
        if (copiedBlock === n) copiedBlock = null;
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

<div class="space-y-2">
  {#each blocks as b (b.n)}
    <div class="rounded-md border p-3" style="background: var(--bg-card); border-color: var(--hairline);">
      <div class="mb-2 flex items-center justify-between">
        <span class="text-[11px] uppercase tracking-wide" style="color: var(--mute);">{#if b.title}{b.title}{/if}</span>
        <button
          class="btn"
          type="button"
          onclick={() => copy(b.n, b.cmd)}
        >
          {copiedBlock === b.n ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre class="log-view overflow-x-auto whitespace-pre text-xs" style="color: var(--body);">{b.cmd}</pre>
    </div>
  {/each}
</div>

<div class="mt-8 text-xs" style="color: var(--mute);">
  paur's GPG key is generated locally on the server and is not
  published to any keyserver, so the very first <code>pacman -U</code>
  cannot validate the keyring package's signature until you've
  manually added the key (step 1) and installed the keyring
  package (step 2). After that, the keyring's post-install hook
  re-imports the pubkey at full trust, so subsequent
  <code>pacman -U</code> and <code>pacman -Sy</code> invocations
  on <code>paur-*</code> packages are signature-validated
  automatically.
</div>
