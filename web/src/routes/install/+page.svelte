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

  const script = $derived(
    `# Bootstrap an Arch client against this paur server.

# 1. Find this server's GPG fingerprint.
FPR=$(curl -sSL ${`${location.protocol}//${host}/repo/${arch}`}/paur.pubkey.asc \\
        | gpg --import-options show-only --import 2>/dev/null \\
        | awk '/^pub/{print $2}' | head -1 | cut -d/ -f2)
echo "FPR=$FPR"

# 2. Bootstrap: add the pubkey to the local keyring and lsign it.
#    --lsign-key prompts for the local keyring's passphrase,
#    which defaults to empty on a fresh install.
sudo pacman-key --add <(curl -sSL ${`${location.protocol}//${host}/repo/${arch}`}/paur.pubkey.asc)
sudo pacman-key --lsign-key "$FPR"

# 3. Install the keyring + mirrorlist meta-packages. The
#    post-install hook on \`paur-keyring\` runs
#    \`pacman-key --populate paur\`, which re-imports the pubkey
#    and lsigns every FPR in \`paur-trusted\` at full trust.
sudo pacman -U --noconfirm '${`${location.protocol}//${host}/repo/${arch}`}/paur-keyring-1-1-any.pkg.tar.zst'
sudo pacman -U --noconfirm '${`${location.protocol}//${host}/repo/${arch}`}/paur-mirrorlist-1-1-any.pkg.tar.zst'

# 4. Enable the repo in pacman.conf and sync.
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[paur]
Include = /etc/pacman.d/paur-mirrorlist
EOF
sudo pacman -Sy hello-pkg`
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
