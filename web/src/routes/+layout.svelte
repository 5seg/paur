<script lang="ts">
  import '../app.css';
  import { page } from '$app/stores';

  let { children } = $props();
</script>

<div class="min-h-screen flex flex-col">
  <header class="border-b border-gray-200 bg-white">
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
            class="rounded px-2 py-1 hover:bg-gray-100 {$page.url.pathname === item.href || ($page.url.pathname.startsWith(item.href) && item.href !== '/') ? 'bg-gray-100 font-medium' : ''}"
          >
            {item.label}
          </a>
        {/each}
      </nav>
      <div class="ml-auto text-xs text-gray-500">personal AUR pre-build</div>
    </div>
  </header>

  <main class="flex-1 mx-auto w-full max-w-6xl px-4 py-6">
    {@render children()}
  </main>

  <footer class="border-t border-gray-200 bg-white">
    <div class="mx-auto max-w-6xl px-4 py-3 text-xs text-gray-500">
      paur · self-hosted AUR pre-build
    </div>
  </footer>
</div>
