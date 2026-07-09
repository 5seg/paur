<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package } from '$lib/api';

  let pkgs = $state<Package[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(true);

  let newName = $state('');
  let newAutoRebuild = $state(false);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);

  async function refresh() {
    try {
      pkgs = await api.listPackages();
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function submit(e: Event) {
    e.preventDefault();
    if (!newName.trim()) return;
    submitting = true;
    submitError = null;
    try {
      await api.addPackage(newName.trim(), newAutoRebuild);
      newName = '';
      newAutoRebuild = false;
      await refresh();
    } catch (err) {
      submitError = err instanceof Error ? err.message : String(err);
    } finally {
      submitting = false;
    }
  }

  async function rebuild(name: string) {
    try {
      await api.rebuildPackage(name);
      await refresh();
    } catch (e) {
      alert(`rebuild failed: ${e}`);
    }
  }

  async function remove(name: string) {
    if (!confirm(`Remove ${name}? This deletes the package and revokes it from the repo.`)) return;
    try {
      await api.removePackage(name);
      await refresh();
    } catch (e) {
      alert(`remove failed: ${e}`);
    }
  }

  onMount(refresh);
</script>

<h1 class="text-2xl font-semibold mb-6">Packages</h1>

<form
  onsubmit={submit}
  class="mb-6 flex flex-wrap items-end gap-3 rounded-md border border-gray-200 bg-white p-4"
>
  <label class="block">
    <span class="text-xs font-medium text-gray-700">AUR package name</span>
    <input
      type="text"
      bind:value={newName}
      placeholder="paru-bin"
      class="mt-1 block w-64 rounded-md border border-gray-300 px-2 py-1.5 text-sm"
      required
      pattern="[a-z0-9][a-z0-9._+-]*"
    />
  </label>
  <label class="inline-flex items-center gap-2 text-sm text-gray-700">
    <input type="checkbox" bind:checked={newAutoRebuild} class="rounded" />
    auto-rebuild on AUR HEAD change
  </label>
  <button class="btn btn-primary" type="submit" disabled={submitting}>
    {submitting ? 'Adding…' : 'Add + build'}
  </button>
  {#if submitError}
    <span class="text-sm text-red-700">{submitError}</span>
  {/if}
</form>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 mb-4">
    {error}
  </div>
{/if}

{#if loading}
  <p class="text-gray-500">Loading…</p>
{:else}
  <div class="overflow-x-auto rounded-md border border-gray-200 bg-white">
    <table class="table-base">
      <thead>
        <tr>
          <th>Name</th>
          <th>Auto</th>
          <th>Latest</th>
          <th>Version</th>
          <th>Finished</th>
          <th></th>
        </tr>
      </thead>
      <tbody class="divide-y divide-gray-100">
        {#each pkgs as p (p.id)}
          <tr>
            <td>
              <a class="text-blue-700 hover:underline" href={`/packages/${p.name}`}>
                {p.name}
              </a>
              <div class="text-xs text-gray-500">{p.aur_url}</div>
            </td>
            <td>{p.auto_rebuild ? 'yes' : 'no'}</td>
            <td>
              {#if p.latest_build}
                <span class={`badge badge-${p.latest_build.status}`}>{p.latest_build.status}</span>
              {:else}
                <span class="text-gray-400">—</span>
              {/if}
            </td>
            <td>{p.latest_build?.pkg_version ?? '-'}</td>
            <td>{fmtTs(p.latest_build?.finished_at)}</td>
            <td class="space-x-1 text-right">
              <button class="btn" onclick={() => rebuild(p.name)}>Rebuild</button>
              <button class="btn btn-danger" onclick={() => remove(p.name)}>Remove</button>
            </td>
          </tr>
        {/each}
        {#if pkgs.length === 0}
          <tr>
            <td colspan="6" class="text-gray-500 text-center py-4">No packages yet.</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{/if}
