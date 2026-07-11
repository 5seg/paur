<script lang="ts">
  import { goto } from '$app/navigation';
  import { login, authState, refreshAuth } from '$lib/auth';

  let username = $state('admin');
  let password = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);

  async function submit(e: Event) {
    e.preventDefault();
    submitting = true;
    error = null;
    try {
      await login(username, password);
      password = '';
      await goto('/packages');
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      submitting = false;
    }
  }

  $effect(() => {
    refreshAuth();
  });
</script>

<h1 class="text-2xl font-semibold mb-6">Sign in</h1>

<div class="max-w-sm rounded-md border border-gray-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
  {#if !$authState.passwordSet}
    <div class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-500/40 dark:bg-amber-500/10 dark:text-amber-300">
      No admin password is set on the host. Run
      <code class="rounded bg-amber-100 px-1 dark:bg-amber-500/20 dark:text-amber-200">paur passwd</code> on the build host to
      create one, then refresh this page.
    </div>
  {/if}

  <form onsubmit={submit} class="space-y-4">
    <label class="block">
      <span class="text-xs font-medium text-gray-700 dark:text-slate-300">Username</span>
      <input
        type="text"
        bind:value={username}
        class="mt-1 block w-full rounded-md border border-gray-300 px-2 py-1.5 text-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
        required
        autocomplete="username"
      />
    </label>
    <label class="block">
      <span class="text-xs font-medium text-gray-700 dark:text-slate-300">Password</span>
      <input
        type="password"
        bind:value={password}
        class="mt-1 block w-full rounded-md border border-gray-300 px-2 py-1.5 text-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
        required
        autocomplete="current-password"
      />
    </label>
    {#if error}
      <div class="text-sm text-red-700 dark:text-red-400">{error}</div>
    {/if}
    <button
      type="submit"
      class="btn btn-primary w-full"
      disabled={submitting || !$authState.passwordSet}
    >
      {submitting ? 'Signing in…' : 'Sign in'}
    </button>
  </form>
</div>
