<script lang="ts">
  import { page } from '$app/stores';
  import { onMount, onDestroy } from 'svelte';
  import { api, fmtTs, streamLogs, type Build } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

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
      loadInitial();
    });
    es.onerror = () => {
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
    es = null;
  });

  async function refresh() {
    await loadInitial();
  }
</script>

<div class="mb-4 flex items-center gap-3">
  <a href="/queue" class="text-sm" style="color: var(--body);">← back</a>
  <h1 class="text-2xl font-semibold tracking-tight" style="color: var(--ink);">
    {#if build}build #{build.seq}{:else}Build #{id}{/if}
  </h1>
  {#if build}
    <StatusBadge status={build.status} />
    <span class="text-sm" style="color: var(--mute);">trigger: {build.trigger}</span>
  {/if}
  <div class="ml-auto flex items-center gap-3">
    <button class="btn" onclick={refresh}>Refresh</button>
    <span class="text-xs" style="color: var(--mute);">{streaming ? 'live' : 'cached'}</span>
  </div>
</div>

{#if error}
  <div class="mb-6 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    {error}
  </div>
{/if}

{#if build}
  {#if build.status === 'running'}
    <div class="progress-bar mb-4"></div>
  {/if}

  <div class="card-vercel mb-6 grid grid-cols-2 gap-4 p-4 text-sm md:grid-cols-4">
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">queued</div>
      <div style="color: var(--body);">{fmtTs(build.queued_at)}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">started</div>
      <div style="color: var(--body);">{fmtTs(build.started_at)}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">finished</div>
      <div style="color: var(--body);">{fmtTs(build.finished_at)}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">exit</div>
      <div style="color: var(--body);">{build.exit_code ?? '—'}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">version</div>
      <div class="font-mono" style="color: var(--body);">{build.pkg_version ?? '—'}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">file</div>
      <div class="font-mono" style="color: var(--body);">{build.pkg_file ?? '—'}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">worker</div>
      <div class="font-mono" style="color: var(--body);">{build.worker_id ?? '—'}</div>
    </div>
    <div>
      <div class="text-[11px] font-medium uppercase tracking-wider" style="color: var(--mute);">trigger</div>
      <div style="color: var(--body);">{build.trigger}</div>
    </div>
  </div>
{/if}

<div bind:this={logEl} class="log-view">
  {#if logLines.length === 0}
    <div style="color: var(--mute);">No log lines yet…</div>
  {:else}
    {#each logLines as line}
      <div class="whitespace-pre-wrap leading-relaxed">{line}</div>
    {/each}
  {/if}
</div>
