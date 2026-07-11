<script lang="ts">
  let { status, animate = true }: { status: string; animate?: boolean } = $props();

  const safe = $derived(
    ['queued', 'running', 'success', 'failed', 'cancelled'].includes(status) ? status : 'queued'
  );
</script>

<span class="badge badge-{safe}">
  <span
    class="status-dot status-dot-{safe}"
    class:animate-pulse-dot={safe === 'running' && animate}
    aria-hidden="true"
  ></span>
  {safe}
</span>
