<script lang="ts">
  import type { Variant } from '$lib/api';

  let { variant, size = 'sm' }: { variant: Variant; size?: 'sm' | 'md' } = $props();

  // Color palette matches the rest of the vercel-style design
  // tokens: default is a neutral slate, v3 is amber, v4 is
  // purple. The bg-*/border-*/text-* triples are picked from
  // Tailwind's 700/50/200 stops so the chip stays legible on
  // both `var(--bg-page)` and `var(--bg-elevated)`.
  const styles: Record<Variant, { bg: string; border: string; text: string; label: string }> = {
    default: {
      bg: 'rgba(100, 116, 139, 0.12)',
      border: 'rgba(100, 116, 139, 0.35)',
      text: 'rgb(203, 213, 225)',
      label: 'default'
    },
    v3: {
      bg: 'rgba(245, 158, 11, 0.12)',
      border: 'rgba(245, 158, 11, 0.35)',
      text: 'rgb(252, 211, 77)',
      label: 'v3'
    },
    v4: {
      bg: 'rgba(168, 85, 247, 0.12)',
      border: 'rgba(168, 85, 247, 0.35)',
      text: 'rgb(216, 180, 254)',
      label: 'v4'
    }
  };
  const s = $derived(styles[variant]);
  const sizeClass = $derived(
    size === 'md' ? 'px-2 py-0.5 text-xs' : 'px-1.5 py-0.5 text-[10px]'
  );
</script>

<span
  class="inline-flex items-center rounded-md border font-mono font-medium {sizeClass}"
  style="background: {s.bg}; border-color: {s.border}; color: {s.text};"
  title="variant: {s.label}"
>
  {s.label}
</span>
