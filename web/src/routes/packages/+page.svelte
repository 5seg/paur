<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api, ApiError, fmtTs, type Package } from '$lib/api';
  import { authState } from '$lib/auth';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

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

  /// If a write call returns 401 the session is gone or never existed.
  /// Bounce the user to the login page so they can re-auth.
  function maybeRedirectToLogin(e: unknown) {
    if (e instanceof ApiError && e.status === 401) {
      goto('/login');
      return true;
    }
    return false;
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
      if (!maybeRedirectToLogin(err)) {
        submitError = err instanceof Error ? err.message : String(err);
      }
    } finally {
      submitting = false;
    }
  }

  async function rebuild(name: string) {
    try {
      await api.rebuildPackage(name);
      await refresh();
    } catch (e) {
      if (!maybeRedirectToLogin(e)) {
        alert(`rebuild failed: ${e}`);
      }
    }
  }

  async function remove(name: string) {
    if (!confirm(`Remove ${name}? This deletes the package and revokes it from the repo.`)) return;
    try {
      await api.removePackage(name);
      await refresh();
    } catch (e) {
      if (!maybeRedirectToLogin(e)) {
        alert(`remove failed: ${e}`);
      }
    }
  }

  onMount(refresh);
</script>

<h1 class="text-2xl font-semibold mb-6">Packages</h1>

{#if $authState.authenticated}
  <form
    onsubmit={submit}
    class="mb-6 flex flex-wrap items-end gap-3 rounded-md border border-gray-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900"
  >
    <label class="block">
      <span class="text-xs font-medium text-gray-700 dark:text-slate-300">AUR package name</span>
      <input
        type="text"
        bind:value={newName}
        placeholder="paru-bin"
        class="mt-1 block w-64 rounded-md border border-gray-300 px-2 py-1.5 text-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
        required
        pattern="[a-z0-9][a-z0-9._+-]*"
      />
    </label>
    <label class="inline-flex items-center gap-2 text-sm text-gray-700 dark:text-slate-300">
      <input type="checkbox" bind:checked={newAutoRebuild} class="rounded" />
      auto-rebuild on AUR HEAD change
    </label>
    <button class="btn btn-primary" type="submit" disabled={submitting}>
      {submitting ? 'Adding…' : 'Add + build'}
    </button>
    {#if submitError}
      <span class="text-sm text-red-700 dark:text-red-400">{submitError}</span>
    {/if}
  </form>
{:else if $authState.ready}
  <div class="mb-6 rounded-md border border-gray-200 bg-white p-4 text-sm text-gray-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300">
    Sign in to add, rebuild, or remove packages. <a class="text-blue-700 hover:underline dark:text-blue-400" href="/login">Sign in</a>
  </div>
{/if}

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 mb-4 dark:border-red-500/40 dark:bg-red-500/10 dark:text-red-300">
    {error}
  </div>
{/if}

{#if loading}
  <p class="text-gray-500 dark:text-slate-400">Loading…</p>
{:else}
  <div class="overflow-x-auto rounded-md border border-gray-200 bg-white dark:border-slate-800 dark:bg-slate-900">
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
      <tbody class="divide-y divide-gray-100 dark:divide-slate-800">
        {#each pkgs as p (p.id)}
          <tr>
            <td>
              <a class="text-blue-700 hover:underline dark:text-blue-400" href={`/packages/${p.name}`}>
                {p.name}
              </a>
              <div class="text-xs text-gray-500 dark:text-slate-500">{p.aur_url}</div>
            </td>
            <td>{p.auto_rebuild ? 'yes' : 'no'}</td>
            <td>
              {#if p.latest_build}
                <StatusBadge status={p.latest_build.status} />
              {:else}
                <span class="text-gray-400 dark:text-slate-600">—</span>
              {/if}
            </td>
            <td>{p.latest_build?.pkg_version ?? '-'}</td>
            <td>{fmtTs(p.latest_build?.finished_at)}</td>
            <td class="space-x-1 text-right">
              {#if $authState.authenticated}
                <button class="btn" onclick={() => rebuild(p.name)}>Rebuild</button>
                <button class="btn btn-danger" onclick={() => remove(p.name)}>Remove</button>
              {/if}
            </td>
          </tr>
        {/each}
        {#if pkgs.length === 0}
          <tr>
            <td colspan="6" class="text-gray-500 text-center py-4 dark:text-slate-400">No packages yet.</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{/if}
