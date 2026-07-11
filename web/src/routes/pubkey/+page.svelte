<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';

  let key = $state('');
  let error = $state<string | null>(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      key = await api.pubkey();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  });

  function copy() {
    navigator.clipboard.writeText(key).catch(() => {});
  }
</script>

<h1 class="mb-6 text-2xl font-semibold tracking-tight" style="color: var(--ink);">GPG public key</h1>

<p class="mb-3 text-sm" style="color: var(--body);">
  Add this key to your client's trust store. The exact commands differ by
  distribution; on Arch:
</p>

<pre class="overflow-x-auto rounded-lg border p-3 text-xs" style="background: var(--bg-card); border-color: var(--hairline); color: var(--body);">
sudo pacman-key --recv-keys &lt;keyid&gt;
sudo pacman-key --lsign-key &lt;keyid&gt;</pre>

{#if error}
  <div class="mb-6 mt-4 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    {error}
  </div>
{/if}

{#if loading}
  <p class="mt-4" style="color: var(--mute);">Loading…</p>
{:else}
  <div class="mt-4 flex items-start gap-3">
    <pre class="log-view flex-1 whitespace-pre">{key}</pre>
    <button class="btn" onclick={copy}>Copy</button>
  </div>
{/if}
