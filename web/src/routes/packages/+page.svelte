<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api, ApiError, fmtTs, type Package, type Variant } from '$lib/api';
  import { authState } from '$lib/auth';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import VariantBadge from '$lib/components/VariantBadge.svelte';
  import DeploymentTable from '$lib/components/DeploymentTable.svelte';

  let pkgs = $state<Package[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(true);

  let newName = $state('');
  let newAutoRebuild = $state(false);
  // Variant toggles for the Add form. `default` is invariant and
  // not shown — it's always on, the daemon enforces it. Only v3
  // and v4 are user-controllable here; the actual selected
  // Variant[] sent to the API derives from these booleans.
  let newVariantV3 = $state(false);
  let newVariantV4 = $state(false);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);

  function selectedVariants(): Variant[] {
    const v: Variant[] = [];
    if (newVariantV3) v.push('v3');
    if (newVariantV4) v.push('v4');
    return v;
  }

  onMount(refresh);

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
      await api.addPackage(newName.trim(), newAutoRebuild, selectedVariants());
      newName = '';
      newAutoRebuild = false;
      newVariantV3 = false;
      newVariantV4 = false;
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
    if (!confirm(`Remove package "${name}"?`)) return;
    try {
      await api.removePackage(name);
      await refresh();
    } catch (e) {
      if (!maybeRedirectToLogin(e)) {
        alert(`remove failed: ${e}`);
      }
    }
  }

  // Per-package view of the active variant set. Daemons predating
  // the variants migration don't include the field; render
  // default-only in that case.
  function activeVariants(p: Package): Variant[] {
    const v = p.variants;
    if (!v) return ['default'];
    const out: Variant[] = [];
    if (v.default) out.push('default');
    if (v.v3) out.push('v3');
    if (v.v4) out.push('v4');
    return out;
  }

  const columns = [
    { key: 'name', label: 'Package' },
    { key: 'auto', label: 'Auto', class: 'w-20' },
    { key: 'variants', label: 'Variants', class: 'w-40' },
    { key: 'latest', label: 'Latest', class: 'w-28' },
    { key: 'version', label: 'Version', class: 'w-48' },
    { key: 'finished', label: 'Finished', class: 'w-40' },
    { key: 'actions', label: '', class: 'w-40' }
  ];
</script>

<h1 class="mb-6 text-2xl font-semibold tracking-tight" style="color: var(--ink);">Packages</h1>

{#if $authState.authenticated}
  <form onsubmit={submit} class="card-vercel mb-6 flex flex-wrap items-end gap-3 p-4">
    <label class="block">
      <span class="text-xs font-medium" style="color: var(--body);">AUR package name</span>
      <input
        type="text"
        bind:value={newName}
        placeholder="paru-bin"
        class="mt-1 block w-64 rounded-md border px-2 py-1.5 text-sm"
        style="background: var(--bg-page); border-color: var(--hairline); color: var(--ink);"
        required
        pattern="[a-z0-9][a-z0-9._+-]*"
      />
    </label>
    <label class="inline-flex items-center gap-2 text-sm" style="color: var(--body);">
      <input type="checkbox" bind:checked={newAutoRebuild} class="rounded" />
      auto-rebuild on AUR HEAD change
    </label>
    <div class="inline-flex items-center gap-3 text-sm" style="color: var(--body);">
      <span class="text-xs" style="color: var(--mute);">Variants:</span>
      <span
        class="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-[11px] font-mono"
        style="background: rgba(100, 116, 139, 0.12); border-color: rgba(100, 116, 139, 0.35); color: rgb(203, 213, 225);"
        title="default is always on"
      >default</span>
      <label class="inline-flex items-center gap-1.5">
        <input type="checkbox" bind:checked={newVariantV3} class="rounded" />
        <span
          class="rounded-md border px-1.5 py-0.5 text-[10px] font-mono"
          style="background: rgba(245, 158, 11, 0.12); border-color: rgba(245, 158, 11, 0.35); color: rgb(252, 211, 77);"
        >v3</span>
      </label>
      <label class="inline-flex items-center gap-1.5">
        <input type="checkbox" bind:checked={newVariantV4} class="rounded" />
        <span
          class="rounded-md border px-1.5 py-0.5 text-[10px] font-mono"
          style="background: rgba(168, 85, 247, 0.12); border-color: rgba(168, 85, 247, 0.35); color: rgb(216, 180, 254);"
        >v4</span>
      </label>
    </div>
    <button class="btn btn-primary" type="submit" disabled={submitting}>
      {submitting ? 'Adding…' : 'Add + build'}
    </button>
    {#if submitError}
      <span class="text-sm" style="color: var(--error);">{submitError}</span>
    {/if}
  </form>
{:else if $authState.ready}
  <div class="card-vercel mb-6 p-4 text-sm" style="color: var(--body);">
    Sign in to add, rebuild, or remove packages. <a class="link-vercel" href="/login">Sign in</a>
  </div>
{/if}

{#if error}
  <div class="mb-6 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    {error}
  </div>
{/if}

{#if loading}
  <p style="color: var(--mute);">Loading…</p>
{:else}
  <DeploymentTable {columns} rows={pkgs} empty="No packages yet.">
    {#snippet row(p: Package)}
      <tr>
        <td>
          <a href="/packages/{p.name}" class="font-medium" style="color: var(--ink);">{p.name}</a>
          <div class="text-xs" style="color: var(--mute);">{p.aur_url}</div>
        </td>
        <td style="color: var(--body);">{p.auto_rebuild ? 'yes' : 'no'}</td>
        <td>
          <div class="flex flex-wrap gap-1">
            {#each activeVariants(p) as v (v)}
              <VariantBadge variant={v} />
            {/each}
          </div>
        </td>
        <td>
          {#if p.latest_build}
            <StatusBadge status={p.latest_build.status} />
          {:else}
            <span style="color: var(--mute);">—</span>
          {/if}
        </td>
        <td class="font-mono text-xs" style="color: var(--body);">{p.latest_build?.pkg_version ?? '—'}</td>
        <td class="text-xs" style="color: var(--mute);">{fmtTs(p.latest_build?.finished_at)}</td>
        <td class="text-right">
          {#if $authState.authenticated}
            <button class="btn mr-1" onclick={() => rebuild(p.name)}>Rebuild</button>
            <button class="btn btn-danger" onclick={() => remove(p.name)}>Remove</button>
          {/if}
        </td>
      </tr>
    {/snippet}
  </DeploymentTable>
{/if}
