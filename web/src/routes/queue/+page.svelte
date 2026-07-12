<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtTs, type Queue, type Package, type Build } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import VariantBadge from '$lib/components/VariantBadge.svelte';
  import DeploymentTable from '$lib/components/DeploymentTable.svelte';

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

  const columns = [
    { key: 'build', label: 'Build', class: 'w-24' },
    { key: 'package', label: 'Package' },
    { key: 'variant', label: 'Variant', class: 'w-24' },
    { key: 'status', label: 'Status', class: 'w-28' },
    { key: 'trigger', label: 'Trigger', class: 'w-32' },
    { key: 'queued', label: 'Queued', class: 'w-40' }
  ];
</script>

<h1 class="mb-6 text-2xl font-semibold tracking-tight" style="color: var(--ink);">Queue</h1>

{#if error}
  <div class="mb-6 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    {error}
  </div>
{/if}

{#if queue}
  <section class="mb-8">
    <div class="mb-3 flex items-center gap-3">
      <h2 class="text-lg font-semibold tracking-tight" style="color: var(--ink);">Running</h2>
      <span class="text-sm" style="color: var(--mute);">({queue.running.length})</span>
      {#if queue.running.length > 0}
        <span class="h-2 w-2 rounded-full bg-paur-running animate-pulse-dot"></span>
      {/if}
    </div>
    {#if queue.running.length > 0}
      <div class="progress-bar mb-4"></div>
    {/if}
    <DeploymentTable {columns} rows={queue.running} empty="Nothing running.">
      {#snippet row(b: Build)}
        <tr>
          <td class="font-mono text-xs">
            <a href="/builds/{b.id}" class="link-vercel">#{b.id}</a>
          </td>
          <td class="font-medium" style="color: var(--ink);">{pkgName(b.package_id)}</td>
          <td><VariantBadge variant={b.variant} /></td>
          <td><StatusBadge status={b.status} /></td>
          <td style="color: var(--body);">{b.trigger}</td>
          <td class="text-xs" style="color: var(--mute);">{fmtTs(b.queued_at)}</td>
        </tr>
      {/snippet}
    </DeploymentTable>
  </section>

  <section>
    <div class="mb-3 flex items-center gap-3">
      <h2 class="text-lg font-semibold tracking-tight" style="color: var(--ink);">Queued</h2>
      <span class="text-sm" style="color: var(--mute);">({queue.queued.length})</span>
    </div>
    <DeploymentTable {columns} rows={queue.queued} empty="Queue is empty.">
      {#snippet row(b: Build)}
        <tr>
          <td class="font-mono text-xs">
            <a href="/builds/{b.id}" class="link-vercel">#{b.id}</a>
          </td>
          <td class="font-medium" style="color: var(--ink);">{pkgName(b.package_id)}</td>
          <td><VariantBadge variant={b.variant} /></td>
          <td><StatusBadge status={b.status} /></td>
          <td style="color: var(--body);">{b.trigger}</td>
          <td class="text-xs" style="color: var(--mute);">{fmtTs(b.queued_at)}</td>
        </tr>
      {/snippet}
    </DeploymentTable>
  </section>
{/if}
