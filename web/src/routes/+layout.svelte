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

<div class="flex min-h-screen flex-col" style="background: var(--bg-page);">
  <header class="h-14 border-b" style="background: #000; border-color: var(--hairline);">
    <div class="mx-auto flex h-full max-w-6xl items-center gap-6 px-4">
      <a href="/" class="text-[15px] font-semibold tracking-tight" style="color: var(--ink);">paur</a>
      <nav class="flex items-center gap-1 text-[13px]">
        {#each [
          { href: '/', label: 'Dashboard' },
          { href: '/packages', label: 'Packages' },
          { href: '/queue', label: 'Queue' },
          { href: '/pubkey', label: 'Pubkey' }
        ] as item}
          {@const active = $page.url.pathname === item.href || ($page.url.pathname.startsWith(item.href) && item.href !== '/')}
          <a
            href={item.href}
            class="rounded-md px-3 py-1.5 font-medium transition-colors"
            style={active ? 'color: var(--ink); background: var(--bg-card);' : 'color: var(--body);'}
            onmouseenter={(e) => { if (!active) e.currentTarget.style.color = 'var(--ink)'; }}
            onmouseleave={(e) => { if (!active) e.currentTarget.style.color = 'var(--body)'; }}
          >
            {item.label}
          </a>
        {/each}
      </nav>
      <div class="ml-auto flex items-center gap-3 text-xs">
        {#if $authState.ready}
          {#if $authState.authenticated}
            <span style="color: var(--mute);">admin</span>
            <button class="btn" onclick={doLogout}>Sign out</button>
          {:else}
            <a class="rounded-full px-4 py-1.5 text-sm font-medium text-white" style="background: var(--accent);" href="/login">Sign in</a>
          {/if}
        {/if}
      </div>
    </div>
  </header>

  <main class="mx-auto w-full max-w-6xl flex-1 px-4 py-8">
    {@render children()}
  </main>

  <footer class="border-t py-3 text-xs" style="background: #000; border-color: var(--hairline); color: var(--mute);">
    <div class="mx-auto max-w-6xl px-4">
      paur · self-hosted AUR pre-build
    </div>
  </footer>
</div>
