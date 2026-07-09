<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package, type Build } from '$lib/api';

  let name = $derived($page.params.name ?? '');
  let pkg = $state<Package | null>(null);
  let builds = $state<Build[]>([]);
  let error = $state<string | null>(null);

  async function refresh() {
    if (!name) return;
    try {
      [pkg, builds] = await Promise.all([
        api.getPackage(name),
        api.listBuilds({ pkg: name, limit: 20 })
      ]);
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    refresh();
    const id = setInterval(refresh, 4000);
    return () => clearInterval(id);
  });

  async function rebuild() {
    if (!pkg) return;
    try {
      await api.rebuildPackage(pkg.name);
      await refresh();
    } catch (e) {
      alert(`rebuild failed: ${e}`);
    }
  }
</script>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800">
    {error}
  </div>
{/if}

{#if pkg}
  <div class="mb-6 flex items-start justify-between">
    <div>
      <h1 class="text-2xl font-semibold">{pkg.name}</h1>
      <p class="text-sm text-gray-600">{pkg.aur_url}</p>
      <p class="text-xs text-gray-500 mt-1">
        auto_rebuild: {pkg.auto_rebuild ? 'yes' : 'no'} ·
        last ref: {pkg.last_known_ref ?? '-'}
      </p>
    </div>
    <button class="btn btn-primary" onclick={rebuild}>Rebuild</button>
  </div>

  <h2 class="text-lg font-semibold mb-2">Latest build</h2>
  {#if pkg.latest_build}
    <div class="rounded-md border border-gray-200 bg-white p-3 mb-6 text-sm">
      <div>
        status: <span class={`badge badge-${pkg.latest_build.status}`}>{pkg.latest_build.status}</span>
      </div>
      <div>version: {pkg.latest_build.pkg_version ?? '-'}</div>
      <div>exit: {pkg.latest_build.exit_code ?? '-'}</div>
      <div>finished: {fmtTs(pkg.latest_build.finished_at)}</div>
      <a class="text-blue-700 hover:underline" href={`/builds/${pkg.latest_build.id}`}>
        view build →
      </a>
    </div>
  {:else}
    <p class="text-gray-500 mb-6">No builds yet.</p>
  {/if}

  <h2 class="text-lg font-semibold mb-2">Recent builds</h2>
  <div class="overflow-x-auto rounded-md border border-gray-200 bg-white">
    <table class="table-base">
      <thead>
        <tr>
          <th>ID</th>
          <th>Status</th>
          <th>Trigger</th>
          <th>Version</th>
          <th>Exit</th>
          <th>Queued</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-gray-100">
        {#each builds as b (b.id)}
          <tr>
            <td>
              <a class="text-blue-700 hover:underline" href={`/builds/${b.id}`}>#{b.id}</a>
            </td>
            <td><span class={`badge badge-${b.status}`}>{b.status}</span></td>
            <td>{b.trigger}</td>
            <td>{b.pkg_version ?? '-'}</td>
            <td>{b.exit_code ?? '-'}</td>
            <td>{fmtTs(b.queued_at)}</td>
          </tr>
        {/each}
        {#if builds.length === 0}
          <tr>
            <td colspan="6" class="text-gray-500 text-center py-4">No builds yet.</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{:else if !error}
  <p class="text-gray-500">Loading…</p>
{/if}
