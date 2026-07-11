<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api, fmtTs, type Package, type Build, type PackageBuildFlags } from '$lib/api';
  import { authState } from '$lib/auth';
  import { goto } from '$app/navigation';
  import { ApiError } from '$lib/api';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

  let name = $derived($page.params.name ?? '');
  let pkg = $state<Package | null>(null);
  let builds = $state<Build[]>([]);
  let error = $state<string | null>(null);
  let togglingAuto = $state(false);
  /// Per-flag pending state for the Build flags section. A non-null
  /// value disables that toggle until the PATCH resolves, preventing
  /// double-clicks from racing.
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

  /// Flip the auto_rebuild flag in-place. If the call 401s, the
  /// session is gone — send the user to /login.
  async function toggleAuto() {
    if (!pkg || togglingAuto) return;
    togglingAuto = true;
    const next = !pkg.auto_rebuild;
    // Optimistic update so the toggle feels instant.
    const prev = pkg.auto_rebuild;
    pkg = { ...pkg, auto_rebuild: next };
    try {
      pkg = await api.setAutoRebuild(pkg.name, next);
    } catch (e) {
      // Roll back on failure.
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

  /// Per-package build flag toggle. The PATCH endpoint treats a
  /// `true` field as "enable" and any other value as "leave alone",
  /// so we send a single-field object. The optimistic update flips
  /// the bit immediately; on failure we roll back.
  async function toggleFlag(key: keyof PackageBuildFlags) {
    if (!pkg || pendingFlag[key]) return;
    const prev = currentFlags();
    const next: PackageBuildFlags = { ...prev, [key]: !prev[key] };
    pendingFlag = { ...pendingFlag, [key]: true };
    pkg = { ...pkg, build_flags: next };
    try {
      // Only send the toggled field as `true` so other flags
      // stay as they are (we never need to send `false` since the
      // endpoint is one-way).
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
</script>

{#if error}
  <div class="rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 dark:border-red-500/40 dark:bg-red-500/10 dark:text-red-300">
    {error}
  </div>
{/if}

{#if pkg}
  <div class="mb-6 flex items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-semibold">{pkg.name}</h1>
      <p class="text-sm text-gray-600 dark:text-slate-400">{pkg.aur_url}</p>
      <p class="text-xs text-gray-500 mt-1 dark:text-slate-500">
        last ref: {pkg.last_known_ref ?? '-'}
      </p>
      <div class="mt-2 flex items-center gap-2 text-sm">
        <span class="text-gray-600 dark:text-slate-400">auto_rebuild:</span>
        {#if $authState.authenticated}
          <button
            type="button"
            onclick={toggleAuto}
            disabled={togglingAuto}
            aria-pressed={pkg.auto_rebuild}
            class={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors ${
              pkg.auto_rebuild
                ? 'border-green-300 bg-green-50 text-green-800 hover:bg-green-100 dark:border-green-500/40 dark:bg-green-500/10 dark:text-green-300 dark:hover:bg-green-500/20'
                : 'border-gray-300 bg-white text-gray-700 hover:bg-gray-50 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700'
            }`}
          >
            <span
              class={`inline-block h-2 w-2 rounded-full ${
                pkg.auto_rebuild ? 'bg-green-500' : 'bg-gray-400 dark:bg-slate-500'
              }`}
            ></span>
            {pkg.auto_rebuild ? 'yes' : 'no'}
          </button>
          <span class="text-xs text-gray-400 dark:text-slate-500">(click to toggle)</span>
        {:else}
          <span class="text-gray-700 dark:text-slate-300">{pkg.auto_rebuild ? 'yes' : 'no'}</span>
        {/if}
      </div>
    </div>
    <button class="btn btn-primary" onclick={rebuild}>Rebuild</button>
  </div>

  <h2 class="text-lg font-semibold mb-2">Build flags</h2>
  <p class="text-xs text-gray-500 mb-3 dark:text-slate-400">
    Per-package build tuning. Changes apply to the next build; the running
    build is unaffected. Send <code class="dark:bg-slate-800 dark:text-slate-200">true</code> to enable, leave the rest alone.
  </p>
  <div class="rounded-md border border-gray-200 bg-white p-3 mb-6 text-sm space-y-2 dark:border-slate-800 dark:bg-slate-900">
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
            class={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors ${
              on
                ? 'border-amber-300 bg-amber-50 text-amber-800 hover:bg-amber-100 dark:border-amber-500/40 dark:bg-amber-500/10 dark:text-amber-300 dark:hover:bg-amber-500/20'
                : 'border-gray-300 bg-white text-gray-700 hover:bg-gray-50 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700'
            }`}
          >
            <span
              class={`inline-block h-2 w-2 rounded-full ${on ? 'bg-amber-500' : 'bg-gray-400 dark:bg-slate-500'}`}
            ></span>
            {on ? 'on' : 'off'}
          </button>
        {:else}
          <span class={`inline-block h-2 w-2 rounded-full mt-1.5 ${on ? 'bg-amber-500' : 'bg-gray-400 dark:bg-slate-500'}`}></span>
          <span class="text-xs text-gray-600 dark:text-slate-400 w-8">{on ? 'on' : 'off'}</span>
        {/if}
        <div class="flex-1">
          <div class="font-medium text-gray-800 dark:text-slate-200">{label}</div>
          <div class="text-xs text-gray-500 dark:text-slate-400">{hint}</div>
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

  <h2 class="text-lg font-semibold mb-2">Latest build</h2>
  {#if pkg.latest_build}
    <div class="rounded-md border border-gray-200 bg-white p-3 mb-6 text-sm dark:border-slate-800 dark:bg-slate-900">
      <div>
        status: <StatusBadge status={pkg.latest_build.status} />
        · <span class="text-gray-600 dark:text-slate-400">build #{pkg.latest_build.seq}</span>
        (id {pkg.latest_build.id})
      </div>
      <div>version: {pkg.latest_build.pkg_version ?? '-'}</div>
      <div>exit: {pkg.latest_build.exit_code ?? '-'}</div>
      <div>finished: {fmtTs(pkg.latest_build.finished_at)}</div>
      <a class="text-blue-700 hover:underline dark:text-blue-400" href={`/builds/${pkg.latest_build.id}`}>
        view build →
      </a>
    </div>
  {:else}
    <p class="text-gray-500 mb-6 dark:text-slate-400">No builds yet.</p>
  {/if}

  <h2 class="text-lg font-semibold mb-2">Recent builds</h2>
  <div class="overflow-x-auto rounded-md border border-gray-200 bg-white dark:border-slate-800 dark:bg-slate-900">
    <table class="table-base">
      <thead>
        <tr>
          <th>#</th>
          <th>Status</th>
          <th>Trigger</th>
          <th>Version</th>
          <th>Exit</th>
          <th>Queued</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-gray-100 dark:divide-slate-800">
        {#each builds as b (b.id)}
          <tr>
            <td>
              <a class="text-blue-700 hover:underline dark:text-blue-400" href={`/builds/${b.id}`}>#{b.seq}</a>
            </td>
            <td><StatusBadge status={b.status} /></td>
            <td>{b.trigger}</td>
            <td>{b.pkg_version ?? '-'}</td>
            <td>{b.exit_code ?? '-'}</td>
            <td>{fmtTs(b.queued_at)}</td>
          </tr>
        {/each}
        {#if builds.length === 0}
          <tr>
            <td colspan="6" class="text-gray-500 text-center py-4 dark:text-slate-400">No builds yet.</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
{:else if !error}
  <p class="text-gray-500 dark:text-slate-400">Loading…</p>
{/if}
