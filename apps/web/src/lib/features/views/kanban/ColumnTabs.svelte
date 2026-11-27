<!--
  The row of tabs above a board that shows one column at a time: the name of
  every column and how many cards it holds, which one is in view, and a
  press to go to one. With many columns the row scrolls sideways, and keeps
  the tab of the column in view in sight.

  The row is only there where the pane fits a single column; where several
  fit it is not shown (a class, so nothing is swapped when the pane changes).
-->
<script lang="ts">
  import { arrowChoice } from '$lib/components/arrow-choice.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  /** One tab: a column, or what stands after the columns. */
  interface Tab {
    key: string;
    name: string;
    /** Shown after the name. Undefined for none. */
    count: number | undefined;
    /** The id of the panel the tab brings into view. */
    panelId: string;
  }

  interface Props {
    tabs: readonly Tab[];
    /** The place of the tab whose panel is in view. */
    current: number;
    /** Whether the strip is shown; a hidden strip must not force layout with `offsetLeft`. */
    visible?: boolean;
    onShow: (at: number) => void;
  }

  let { tabs, current, visible = true, onShow }: Props = $props();

  let strip = $state<HTMLElement | null>(null);

  $effect(() => {
    if (!visible) return;
    const tab = strip?.children[current];
    if (!strip || !(tab instanceof HTMLElement)) return;
    const left = tab.offsetLeft - 8;
    const right = tab.offsetLeft + tab.offsetWidth + 8;
    if (left < strip.scrollLeft) strip.scrollLeft = left;
    else if (right > strip.scrollLeft + strip.clientWidth) {
      strip.scrollLeft = right - strip.clientWidth;
    }
  });
</script>

<div
  bind:this={strip}
  role="tablist"
  aria-label={m.kanban_columns()}
  {@attach arrowChoice()}
  data-testid="column-tabs"
  class="relative flex shrink-0 [scrollbar-width:none] overflow-x-auto border-b px-2 @columns/pane:hidden"
>
  {#each tabs as tab, at (tab.key)}
    <button
      type="button"
      role="tab"
      aria-selected={at === current}
      aria-controls={tab.panelId}
      data-tab={tab.key}
      class="group/tab flex h-10 shrink-0 items-center px-1 outline-none roomy:h-touch"
      onclick={() => {
        onShow(at);
      }}
    >
      <span
        class={[
          'flex h-8 max-w-48 items-center gap-1.5 rounded-full px-3 text-sm whitespace-nowrap group-focus-visible/tab:ring-2 group-focus-visible/tab:ring-ring',
          at === current
            ? 'bg-primary font-medium text-primary-foreground'
            : 'text-muted-foreground',
        ]}
      >
        <span class="truncate">{tab.name}</span>
        {#if tab.count !== undefined}
          <span class="text-xs opacity-80">{tab.count}</span>
        {/if}
      </span>
    </button>
  {/each}
</div>
