<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package, type Build } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

  let pkgs = $state<Package[]>([]);
  let queue = $state<{ queued: Build[]; running: Build[] } | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);

  async function refresh() {
    try {
      [pkgs, queue] = await Promise.all([api.listPackages(), api.queue()]);
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
    const id = setInterval(refresh, 5000);
    return () => clearInterval(id);
  });

  let total = $derived(pkgs.length);
  let success = $derived(pkgs.filter((p) => p.latest_build?.status === 'success').length);
  let failed = $derived(pkgs.filter((p) => p.latest_build?.status === 'failed').length);
  let running = $derived(queue?.running.length ?? 0);
  let queued = $derived(queue?.queued.length ?? 0);
</script>

<h1 class="text-2xl font-semibold mb-6">Dashboard</h1>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 dark:border-red-500/40 dark:bg-red-500/10 dark:text-red-300">
    Failed to reach the daemon: {error}
  </div>
{/if}

{#if loading}
  <p class="text-gray-500 dark:text-slate-400">Loading…</p>
{:else}
  <div class="grid grid-cols-2 gap-4 md:grid-cols-5">
    {#each [
      { label: 'Packages', value: total },
      { label: 'Latest success', value: success },
      { label: 'Latest failed', value: failed },
      { label: 'Running', value: running },
      { label: 'Queued', value: queued }
    ] as stat}
      <div class="rounded-md border border-gray-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
        <div class="text-xs uppercase text-gray-500 dark:text-slate-400">{stat.label}</div>
        <div class="text-2xl font-semibold">{stat.value}</div>
        {#if stat.label === 'Running' && running > 0}
          <div class="progress-bar mt-2"></div>
        {/if}
      </div>
    {/each}
  </div>

  <h2 class="mt-8 mb-3 text-lg font-semibold">Recent activity</h2>
  <div class="overflow-x-auto rounded-md border border-gray-200 bg-white dark:border-slate-800 dark:bg-slate-900">
    <table class="table-base">
      <thead>
        <tr>
          <th>Package</th>
          <th>Latest status</th>
          <th>Version</th>
          <th>Finished</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-gray-100 dark:divide-slate-800">
        {#each pkgs.slice(0, 10) as p (p.id)}
          <tr>
            <td>
              <a class="text-blue-700 hover:underline dark:text-blue-400" href={`/packages/${p.name}`}>
                {p.name}
              </a>
            </td>
            <td>
              {#if p.latest_build}
                <StatusBadge status={p.latest_build.status} />
              {:else}
                <span class="text-gray-400 dark:text-slate-600">—</span>
              {/if}
            </td>
            <td>{p.latest_build?.pkg_version ?? '-'}</td>
            <td>{fmtTs(p.latest_build?.finished_at)}</td>
          </tr>
        {/each}
        {#if pkgs.length === 0}
          <tr>
            <td colspan="4" class="text-gray-500 text-center py-4 dark:text-slate-400">
              No packages yet. <a href="/packages" class="text-blue-700 dark:text-blue-400">Add one</a>.
            </td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{/if}
