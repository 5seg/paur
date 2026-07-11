<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package, type Build, type PackageBuildFlags } from '$lib/api';
  import { authState } from '$lib/auth';
  import { goto } from '$app/navigation';
  import { ApiError } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import DeploymentTable from '$lib/components/DeploymentTable.svelte';

  let name = $derived($page.params.name ?? '');
  let pkg = $state<Package | null>(null);
  let builds = $state<Build[]>([]);
  let error = $state<string | null>(null);
  let togglingAuto = $state(false);
  let pendingFlag = $state<{ [K in keyof PackageBuildFlags]?: boolean }>({});

  const EMPTY_FLAGS: PackageBuildFlags = {
    low_memory: false,
    rust_codegen_units_1: false,
    no_ccache: false
  };
  function currentFlags(): PackageBuildFlags {
    return pkg?.build_flags ?? EMPTY_FLAGS;
  }

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
      if (e instanceof ApiError && e.status === 401) {
        goto('/login');
        return;
      }
      alert(`rebuild failed: ${e}`);
    }
  }

  async function toggleAuto() {
    if (!pkg || togglingAuto) return;
    togglingAuto = true;
    const next = !pkg.auto_rebuild;
    const prev = pkg.auto_rebuild;
    pkg = { ...pkg, auto_rebuild: next };
    try {
      pkg = await api.setAutoRebuild(pkg.name, next);
    } catch (e) {
      pkg = pkg ? { ...pkg, auto_rebuild: prev } : pkg;
      if (e instanceof ApiError && e.status === 401) {
        goto('/login');
        return;
      }
      alert(`auto toggle failed: ${e}`);
    } finally {
      togglingAuto = false;
    }
  }

  async function toggleFlag(key: keyof PackageBuildFlags) {
    if (!pkg || pendingFlag[key]) return;
    const prev = currentFlags();
    const next: PackageBuildFlags = { ...prev, [key]: !prev[key] };
    pendingFlag = { ...pendingFlag, [key]: true };
    pkg = { ...pkg, build_flags: next };
    try {
      pkg = await api.setBuildFlags(pkg.name, { [key]: true } as Partial<PackageBuildFlags>);
    } catch (e) {
      pkg = pkg ? { ...pkg, build_flags: prev } : pkg;
      if (e instanceof ApiError && e.status === 401) {
        goto('/login');
        return;
      }
      alert(`flag toggle failed: ${e}`);
    } finally {
      const { [key]: _drop, ...rest } = pendingFlag;
      pendingFlag = rest;
    }
  }

  const buildColumns = [
    { key: 'num', label: '#', class: 'w-20' },
    { key: 'status', label: 'Status', class: 'w-28' },
    { key: 'trigger', label: 'Trigger', class: 'w-32' },
    { key: 'version', label: 'Version', class: 'w-48' },
    { key: 'exit', label: 'Exit', class: 'w-20' },
    { key: 'queued', label: 'Queued', class: 'w-40' }
  ];
</script>

{#if error}
  <div class="mb-6 rounded-lg border border-red-500/30 bg-red-500/10 p-3 text-sm" style="color: var(--error);">
    {error}
  </div>
{/if}

{#if pkg}
  <div class="mb-6 flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight" style="color: var(--ink);">{pkg.name}</h1>
      <p class="text-sm" style="color: var(--mute);">{pkg.aur_url}</p>
      <p class="mt-1 text-xs" style="color: var(--mute);">
        last ref: {pkg.last_known_ref ?? '-'}
      </p>
      <div class="mt-2 flex items-center gap-2 text-sm">
        <span style="color: var(--body);">auto_rebuild:</span>
        {#if $authState.authenticated}
          <button
            type="button"
            onclick={toggleAuto}
            disabled={togglingAuto}
            aria-pressed={pkg.auto_rebuild}
            class="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
            style={pkg.auto_rebuild
              ? 'background: rgba(34, 197, 94, 0.12); border-color: rgba(34, 197, 94, 0.35); color: var(--success);'
              : 'background: var(--bg-elevated); border-color: var(--hairline); color: var(--body);'}
          >
            <span
              class="inline-block h-2 w-2 rounded-full"
              style="background: {pkg.auto_rebuild ? 'var(--success)' : 'var(--mute)'};"
            ></span>
            {pkg.auto_rebuild ? 'yes' : 'no'}
          </button>
          <span class="text-xs" style="color: var(--mute);">(click to toggle)</span>
        {:else}
          <span style="color: var(--body);">{pkg.auto_rebuild ? 'yes' : 'no'}</span>
        {/if}
      </div>
    </div>
    <button class="btn btn-primary" onclick={rebuild}>Rebuild</button>
  </div>

  <h2 class="mb-2 text-lg font-semibold tracking-tight" style="color: var(--ink);">Build flags</h2>
  <p class="mb-3 text-xs" style="color: var(--mute);">
    Per-package build tuning. Changes apply to the next build; the running
    build is unaffected. Send <code class="rounded px-1" style="background: var(--bg-elevated); color: var(--body);">true</code> to enable, leave the rest alone.
  </p>
  <div class="card-vercel mb-6 space-y-2 p-3 text-sm">
    {#snippet flagRow(key: keyof PackageBuildFlags, label: string, hint: string)}
      {@const on = currentFlags()[key]}
      {@const busy = !!pendingFlag[key]}
      <div class="flex items-start gap-3">
        {#if $authState.authenticated}
          <button
            type="button"
            onclick={() => toggleFlag(key)}
            disabled={busy}
            aria-pressed={on}
            class="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
            style={on
              ? 'background: rgba(245, 158, 11, 0.12); border-color: rgba(245, 158, 11, 0.35); color: var(--warning);'
              : 'background: var(--bg-elevated); border-color: var(--hairline); color: var(--body);'}
          >
            <span
              class="inline-block h-2 w-2 rounded-full"
              style="background: {on ? 'var(--warning)' : 'var(--mute)'};"
            ></span>
            {on ? 'on' : 'off'}
          </button>
        {:else}
          <span class="mt-1.5 inline-block h-2 w-2 rounded-full" style="background: {on ? 'var(--warning)' : 'var(--mute)'};"></span>
          <span class="w-8 text-xs" style="color: var(--body);">{on ? 'on' : 'off'}</span>
        {/if}
        <div class="flex-1">
          <div class="font-medium" style="color: var(--ink);">{label}</div>
          <div class="text-xs" style="color: var(--mute);">{hint}</div>
        </div>
      </div>
    {/snippet}
    {@render flagRow(
      'low_memory',
      'Low memory',
      'Cap parallel make jobs to -j2 to cut peak RAM. Use for OOM-prone packages (wayvr, llvm, firefox).'
    )}
    {@render flagRow(
      'rust_codegen_units_1',
      'Rust codegen-units=1',
      'Append -C codegen-units=1 to RUSTFLAGS. ~20-30% lower rustc memory, slower codegen. Only affects Rust packages.'
    )}
    {@render flagRow(
      'no_ccache',
      'No ccache',
      "Skip the ccache bind mount. Use when ccache misses dominate or the cache dir is unhelpfully large."
    )}
  </div>

  <h2 class="mb-2 text-lg font-semibold tracking-tight" style="color: var(--ink);">Latest build</h2>
  {#if pkg.latest_build}
    <div class="card-vercel mb-6 p-3 text-sm">
      <div class="mb-1">
        <StatusBadge status={pkg.latest_build.status} />
        <span class="ml-2" style="color: var(--body);">build #{pkg.latest_build.seq} (id {pkg.latest_build.id})</span>
      </div>
      <div style="color: var(--body);">version: <span class="font-mono">{pkg.latest_build.pkg_version ?? '—'}</span></div>
      <div style="color: var(--body);">exit: {pkg.latest_build.exit_code ?? '—'}</div>
      <div style="color: var(--body);">finished: {fmtTs(pkg.latest_build.finished_at)}</div>
      <a href="/builds/{pkg.latest_build.id}" class="link-vercel text-sm">view build →</a>
    </div>
  {:else}
    <p class="mb-6 text-sm" style="color: var(--mute);">No builds yet.</p>
  {/if}

  <h2 class="mb-3 text-lg font-semibold tracking-tight" style="color: var(--ink);">Recent builds</h2>
  <DeploymentTable columns={buildColumns} rows={builds} empty="No builds yet.">
    {#snippet row(b: Build)}
      <tr>
        <td class="font-mono text-xs">
          <a href="/builds/{b.id}" class="link-vercel">#{b.seq}</a>
        </td>
        <td><StatusBadge status={b.status} /></td>
        <td style="color: var(--body);">{b.trigger}</td>
        <td class="font-mono text-xs" style="color: var(--body);">{b.pkg_version ?? '—'}</td>
        <td style="color: var(--body);">{b.exit_code ?? '—'}</td>
        <td class="text-xs" style="color: var(--mute);">{fmtTs(b.queued_at)}</td>
      </tr>
    {/snippet}
  </DeploymentTable>
{:else if !error}
  <p style="color: var(--mute);">Loading…</p>
{/if}
