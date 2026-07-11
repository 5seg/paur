<script lang="ts">
  import '../app.css';
  import { page } from '$app/stores';
  import { authState, refreshAuth, logout } from '$lib/auth';
  import { onMount } from 'svelte';

  let { children } = $props();

  onMount(refreshAuth);

  async function doLogout() {
    await logout();
  }
</script>

<div class="min-h-screen flex flex-col">
  <header class="border-b border-slate-800 bg-slate-900">
    <div class="mx-auto max-w-6xl px-4 py-3 flex items-center gap-6">
      <a href="/" class="text-xl font-semibold tracking-tight">paur</a>
      <nav class="flex gap-4 text-sm">
        {#each [
          { href: '/', label: 'Dashboard' },
          { href: '/packages', label: 'Packages' },
          { href: '/queue', label: 'Queue' },
          { href: '/pubkey', label: 'Pubkey' }
        ] as item}
          <a
            href={item.href}
            class="rounded px-2 py-1 hover:bg-slate-800 {$page.url.pathname === item.href || ($page.url.pathname.startsWith(item.href) && item.href !== '/') ? 'bg-slate-800 font-medium' : ''}"
          >
            {item.label}
          </a>
        {/each}
      </nav>
      <div class="ml-auto flex items-center gap-3 text-xs">
        {#if $authState.ready}
          {#if $authState.authenticated}
            <span class="text-slate-400">admin</span>
            <button class="btn" onclick={doLogout}>Sign out</button>
          {:else}
            <a class="btn btn-primary" href="/login">Sign in</a>
          {/if}
        {/if}
      </div>
    </div>
  </header>

  <main class="flex-1 mx-auto w-full max-w-6xl px-4 py-6">
    {@render children()}
  </main>

  <footer class="border-t border-slate-800 bg-slate-900">
    <div class="mx-auto max-w-6xl px-4 py-3 text-xs text-slate-400">
      paur · self-hosted AUR pre-build
    </div>
  </footer>
</div>
