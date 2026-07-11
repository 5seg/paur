<script lang="ts" generics="T extends { id: number | string }">
  import { type Snippet } from 'svelte';

  let {
    columns,
    rows,
    empty,
    row
  }: {
    columns: { key: string; label: string; class?: string }[];
    rows: T[];
    empty: string;
    row: Snippet<[T]>;
  } = $props();
</script>

<div class="card-vercel overflow-hidden">
  <table class="table-base">
    <thead>
      <tr>
        {#each columns as col}
          <th class={col.class ?? ''}>{col.label}</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as item (item.id)}
        {@render row(item)}
      {/each}
      {#if rows.length === 0}
        <tr>
          <td colspan={columns.length} class="py-8 text-center text-sm" style="color: var(--mute);">
            {empty}
          </td>
        </tr>
      {/if}
    </tbody>
  </table>
</div>
