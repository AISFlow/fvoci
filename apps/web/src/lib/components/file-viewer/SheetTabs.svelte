<!--
  The sheets of a workbook in the viewer, by name: a row of tabs under the
  sheet, as a spreadsheet has them, that scrolls across on its own when the
  names are more than the width. The sheet shown is the one marked current,
  and kept in view; pressing a name shows its sheet. A sheet with charts or
  pictures, which the viewer does not draw, says so above the row.
-->
<script lang="ts">
  import type { SheetSummary } from '$lib/file-viewer/protocol.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Props {
    sheets: readonly SheetSummary[];
    /** The sheet shown, from 1. */
    current: number;
    onPick: (sheet: number) => void;
  }

  let { sheets, current, onPick }: Props = $props();

  const drawings = $derived(sheets[current - 1]?.drawings ?? false);

  /** Keeps the current sheet's tab in view as the sheet changes. */
  function keptInView(active: boolean) {
    return (tab: HTMLElement) => {
      if (active) tab.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    };
  }
</script>

{#if drawings}
  <p class="border-t px-4 py-1.5 text-sm text-muted-foreground" data-testid="file-viewer-drawings">
    {m.viewer_sheet_drawings()}
  </p>
{/if}
<div
  class="flex min-h-10 items-stretch gap-1 overflow-x-auto border-t px-2 py-1"
  role="group"
  aria-label={m.viewer_sheets()}
  data-testid="file-viewer-sheets"
>
  {#each sheets as sheet, index (sheet)}
    {@const active = index + 1 === current}
    <button
      type="button"
      class={[
        'max-w-48 shrink-0 truncate rounded-md px-3 text-sm whitespace-nowrap roomy:min-h-touch',
        active
          ? 'bg-accent font-medium text-accent-foreground'
          : 'text-muted-foreground hover:bg-muted hover:text-foreground',
      ]}
      aria-current={active ? 'true' : undefined}
      title={sheet.name}
      onclick={() => {
        onPick(index + 1);
      }}
      {@attach keptInView(active)}
    >
      {sheet.name}
    </button>
  {/each}
</div>
