<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package, type Build } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import StatCard from '$lib/components/StatCard.svelte';
  import DeploymentTable from '$lib/components/DeploymentTable.svelte';

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
  let successRate = $derived(total > 0 ? Math.round((success / total) * 100) : 0);

  const columns = [
    { key: 'status', label: 'Status', class: 'w-24' },
    { key: 'package', label: 'Package' },
    { key: 'version', label: 'Version', class: 'w-48' },
    { key: 'finished', label: 'Finished', class: 'w-40' }
  ];
</script>

<h1 class="mb-6 text-2xl font-semibold tracking-tight" style="color: var(--ink);">Dashboard</h1>

{#if error}
  <div class="mb-6 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    Failed to reach the daemon: {error}
  </div>
{/if}

{#if loading}
  <p style="color: var(--mute);">Loading…</p>
{:else}
  <div class="mb-8 grid grid-cols-1 gap-4 md:grid-cols-3">
    <StatCard
      label="Packages"
      value={total}
      hint="{success} latest success · {failed} failed"
    />
    <StatCard label="Success Rate" value="{successRate}%" hint="{success} of {total} packages">
      <div class="h-1.5 w-full overflow-hidden rounded-full" style="background: var(--hairline);">
        <div
          class="h-full rounded-full transition-all"
          style="width: {successRate}%; background: var(--success);"
        ></div>
      </div>
    </StatCard>
    <StatCard label="Queue" value="{running} / {queued}" hint="Running / Queued">
      {#if running > 0}
        <div class="flex items-center gap-2">
          <span class="h-2 w-2 rounded-full bg-paur-running animate-pulse-dot"></span>
          <span class="text-xs" style="color: var(--body);">{running} build{running === 1 ? '' : 's'} running</span>
        </div>
      {/if}
    </StatCard>
  </div>

  <h2 class="mb-3 text-lg font-semibold tracking-tight" style="color: var(--ink);">Recent activity</h2>

  <DeploymentTable {columns} rows={pkgs.slice(0, 10)} empty="No packages yet.">
    {#snippet row(p: Package)}
      <tr>
        <td>
          {#if p.latest_build}
            <StatusBadge status={p.latest_build.status} />
          {:else}
            <span style="color: var(--mute);">—</span>
          {/if}
        </td>
        <td>
          <a href="/packages/{p.name}" class="font-medium" style="color: var(--ink);">{p.name}</a>
          <div class="text-xs" style="color: var(--mute);">{p.aur_url}</div>
        </td>
        <td class="font-mono text-xs" style="color: var(--body);">{p.latest_build?.pkg_version ?? '—'}</td>
        <td class="text-xs" style="color: var(--mute);">{fmtTs(p.latest_build?.finished_at)}</td>
      </tr>
    {/snippet}
  </DeploymentTable>
{/if}
