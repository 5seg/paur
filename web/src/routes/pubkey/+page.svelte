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

<h1 class="text-2xl font-semibold mb-6">GPG public key</h1>

<p class="text-sm text-gray-700 mb-3">
  Add this key to your client's trust store. The exact commands differ by
  distribution; on Arch:
</p>

<pre class="rounded-md border border-gray-300 bg-gray-900 p-3 text-xs text-gray-100 overflow-x-auto">
sudo pacman-key --recv-keys &lt;keyid&gt;
sudo pacman-key --lsign-key &lt;keyid&gt;</pre>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 mt-4">
    {error}
  </div>
{/if}

{#if loading}
  <p class="text-gray-500 mt-4">Loading…</p>
{:else}
  <div class="mt-4 flex items-start gap-3">
    <pre class="log-view flex-1 whitespace-pre">{key}</pre>
    <button class="btn" onclick={copy}>Copy</button>
  </div>
{/if}
