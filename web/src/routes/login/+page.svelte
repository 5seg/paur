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

<h1 class="mb-6 text-2xl font-semibold tracking-tight" style="color: var(--ink);">Sign in</h1>

<div class="card-vercel max-w-sm p-6">
  {#if !$authState.passwordSet}
    <div class="mb-4 rounded border p-3 text-sm" style="background: rgba(245, 158, 11, 0.1); border-color: rgba(245, 158, 11, 0.3); color: var(--warning);">
      No admin password is set on the host. Run
      <code class="rounded px-1" style="background: rgba(245, 158, 11, 0.15);">paur passwd</code> on the build host to
      create one, then refresh this page.
    </div>
  {/if}

  <form onsubmit={submit} class="space-y-4">
    <label class="block">
      <span class="text-xs font-medium" style="color: var(--body);">Username</span>
      <input
        type="text"
        bind:value={username}
        class="mt-1 block w-full rounded-md border px-2 py-1.5 text-sm"
        style="background: var(--bg-page); border-color: var(--hairline); color: var(--ink);"
        required
        autocomplete="username"
      />
    </label>
    <label class="block">
      <span class="text-xs font-medium" style="color: var(--body);">Password</span>
      <input
        type="password"
        bind:value={password}
        class="mt-1 block w-full rounded-md border px-2 py-1.5 text-sm"
        style="background: var(--bg-page); border-color: var(--hairline); color: var(--ink);"
        required
        autocomplete="current-password"
      />
    </label>
    {#if error}
      <div class="text-sm" style="color: var(--error);">{error}</div>
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
