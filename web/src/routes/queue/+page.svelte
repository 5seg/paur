<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtTs, type Queue, type Package, type Build } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

  let queue = $state<Queue | null>(null);
  let pkgs = $state<Package[]>([]);
  let error = $state<string | null>(null);

  async function refresh() {
    try {
      [queue, pkgs] = await Promise.all([api.queue(), api.listPackages()]);
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function pkgName(packageId: number): string {
    const p = pkgs.find((x) => x.id === packageId);
    return p ? p.name : `#${packageId}`;
  }

  onMount(() => {
    refresh();
    const id = setInterval(refresh, 2000);
    return () => clearInterval(id);
  });
</script>

<h1 class="text-2xl font-semibold mb-6">Queue</h1>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 mb-4 dark:border-red-500/40 dark:bg-red-500/10 dark:text-red-300">
    {error}
  </div>
{/if}

{#if queue}
  <section class="mb-8">
    <h2 class="text-lg font-semibold mb-2">
      Running <span class="text-gray-500 text-sm dark:text-slate-400">({queue.running.length})</span>
    </h2>
    {#if queue.running.length > 0}
      <div class="progress-bar mb-3"></div>
    {/if}
    <div class="overflow-x-auto rounded-md border border-gray-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      <table class="table-base">
        <thead>
          <tr>
            <th>Build</th>
            <th>Package</th>
            <th>Status</th>
            <th>Trigger</th>
            <th>Queued</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-gray-100 dark:divide-slate-800">
          {#each queue.running as b (b.id)}
            <tr>
              <td><a class="text-blue-700 hover:underline dark:text-blue-400" href={`/builds/${b.id}`}>#{b.id}</a></td>
              <td>{pkgName(b.package_id)}</td>
              <td><StatusBadge status={b.status} /></td>
              <td>{b.trigger}</td>
              <td>{fmtTs(b.queued_at)}</td>
            </tr>
          {/each}
          {#if queue.running.length === 0}
            <tr><td colspan="5" class="text-gray-500 text-center py-4 dark:text-slate-400">Nothing running.</td></tr>
          {/if}
        </tbody>
      </table>
    </div>
  </section>

  <section>
    <h2 class="text-lg font-semibold mb-2">
      Queued <span class="text-gray-500 text-sm dark:text-slate-400">({queue.queued.length})</span>
    </h2>
    <div class="overflow-x-auto rounded-md border border-gray-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      <table class="table-base">
        <thead>
          <tr>
            <th>Build</th>
            <th>Package</th>
            <th>Status</th>
            <th>Trigger</th>
            <th>Queued</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-gray-100 dark:divide-slate-800">
          {#each queue.queued as b (b.id)}
            <tr>
              <td><a class="text-blue-700 hover:underline dark:text-blue-400" href={`/builds/${b.id}`}>#{b.id}</a></td>
              <td>{pkgName(b.package_id)}</td>
              <td><StatusBadge status={b.status} /></td>
              <td>{b.trigger}</td>
              <td>{fmtTs(b.queued_at)}</td>
            </tr>
          {/each}
          {#if queue.queued.length === 0}
            <tr><td colspan="5" class="text-gray-500 text-center py-4 dark:text-slate-400">Queue is empty.</td></tr>
          {/if}
        </tbody>
      </table>
    </div>
  </section>
{/if}
