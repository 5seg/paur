<script lang="ts">
  import { page } from '$app/stores';
  import { onMount, onDestroy } from 'svelte';
  import { api, fmtTs, streamLogs, type Build } from '$lib/api';

  let id = $derived(Number($page.params.id));
  let build = $state<Build | null>(null);
  let logLines = $state<string[]>([]);
  let error = $state<string | null>(null);
  let streaming = $state(true);
  let es: EventSource | null = null;
  let logEl: HTMLDivElement | null = $state(null);

  function autoScroll() {
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  }

  async function loadInitial() {
    try {
      build = await api.getBuild(id);
      // Pre-fill with the cached log, if any.
      try {
        const cached = await api.rawLogs(id);
        if (cached) logLines = cached.split('\n');
      } catch {
        // No cached log; that's fine for fresh builds.
      }
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function startStream() {
    if (es) es.close();
    es = streamLogs(id);
    es.onmessage = (ev) => {
      logLines = [...logLines, ev.data];
      autoScroll();
    };
    es.addEventListener('done', () => {
      streaming = false;
      es?.close();
      es = null;
      // Refresh the build row to pick up the final status.
      loadInitial();
    });
    es.onerror = () => {
      // SSE errored (most often: build not running, channel closed).
      // The server also sends a single 'done' event when the stream
      // ends, so we just stop streaming and let the user click
      // "Refresh" to fetch the final cached log.
      streaming = false;
      es?.close();
      es = null;
    };
  }

  onMount(() => {
    loadInitial();
    startStream();
  });

  onDestroy(() => {
    es?.close();
  });

  async function refresh() {
    await loadInitial();
    if (!streaming) startStream();
  }
</script>

<div class="mb-4 flex items-center gap-3">
  <a class="text-sm text-blue-700 hover:underline" href="/queue">← back</a>
  <h1 class="text-2xl font-semibold">Build #{id}</h1>
  {#if build}
    <span class={`badge badge-${build.status}`}>{build.status}</span>
    <span class="text-sm text-gray-500">trigger: {build.trigger}</span>
  {/if}
  <div class="ml-auto space-x-2">
    <button class="btn" onclick={refresh}>Refresh</button>
    <span class="text-xs text-gray-500">
      {streaming ? 'live' : 'cached'}
    </span>
  </div>
</div>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800">
    {error}
  </div>
{/if}

{#if build}
  <div class="mb-4 grid grid-cols-2 gap-3 text-sm md:grid-cols-4">
    <div><span class="text-gray-500">queued:</span> {fmtTs(build.queued_at)}</div>
    <div><span class="text-gray-500">started:</span> {fmtTs(build.started_at)}</div>
    <div><span class="text-gray-500">finished:</span> {fmtTs(build.finished_at)}</div>
    <div><span class="text-gray-500">exit:</span> {build.exit_code ?? '-'}</div>
    <div><span class="text-gray-500">version:</span> {build.pkg_version ?? '-'}</div>
    <div><span class="text-gray-500">file:</span> {build.pkg_file ?? '-'}</div>
    <div><span class="text-gray-500">worker:</span> {build.worker_id ?? '-'}</div>
    <div><span class="text-gray-500">trigger:</span> {build.trigger}</div>
  </div>
{/if}

<div bind:this={logEl} class="log-view">
  {#if logLines.length === 0}
    <div class="text-gray-500">No log lines yet…</div>
  {:else}
    {#each logLines as line, i (i)}
      <div>{line}</div>
    {/each}
  {/if}
</div>
