<script lang="ts">
  import { type Snippet } from 'svelte';

  let {
    label,
    value,
    hint,
    children
  }: {
    label: string;
    value: string | number;
    hint?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="card-vercel p-5">
  <div class="text-xs font-medium uppercase tracking-wider" style="color: var(--mute);">
    {label}
  </div>
  <div class="mt-1 text-[32px] font-semibold leading-none tracking-tight" style="color: var(--ink);">
    {value}
  </div>
  {#if hint}
    <div class="mt-2 text-xs" style="color: var(--mute);">{hint}</div>
  {/if}
  {#if children}
    <div class="mt-3">
      {@render children()}
    </div>
  {/if}
</div>
