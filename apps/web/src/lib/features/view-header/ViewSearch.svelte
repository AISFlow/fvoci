<!--
  The search of a view's cards: an icon, and once pressed a field in its
  place in the whole row, over the whole row in the folded one; open while it
  has text. The field is one element in both rows, so that it keeps what was
  typed, the caret and the focus when the pane changes its width. First among
  the tools: an empty field that gives its place back as the focus leaves
  moves no tool to its right from under the pointer pressing it.
-->
<script lang="ts">
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import SearchIcon from '@lucide/svelte/icons/search';
  import { flushSync } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    /** The search text of the view. Bindable. */
    search: string;
    /** The view whose search field was opened; another view starts closed. Bindable. */
    openIn: string | undefined;
    viewId: string;
    /** The header is one whole row. */
    row: boolean;
  }

  let { search = $bindable(), openIn = $bindable(), viewId, row }: Props = $props();

  /** The search field is open: asked for, or with text. */
  const shown = $derived(openIn === viewId || search !== '');
  /** The folded row shows the search field in place of everything else. */
  const bar = $derived(!row && shown);

  let trigger = $state<HTMLElement | null>(null);
  let field = $state<HTMLInputElement | null>(null);

  function open(): void {
    // Synchronous flush keeps focus inside the click's user activation, with no async gap.
    flushSync(() => {
      openIn = viewId;
    });
    // Force layout recalc after removing `hidden` so WebKit treats the input as focusable at once.
    field?.getBoundingClientRect();
    field?.focus({ preventScroll: true });
    queueMicrotask(() => {
      if (
        shown &&
        field &&
        (document.activeElement === document.body || document.activeElement === trigger)
      ) {
        field.focus({ preventScroll: true });
      }
    });
  }

  function close(): void {
    flushSync(() => {
      search = '';
      openIn = undefined;
    });
    trigger?.getBoundingClientRect();
    trigger?.focus({ preventScroll: true });
    queueMicrotask(() => {
      if (!shown && trigger && document.activeElement === document.body) {
        trigger.focus({ preventScroll: true });
      }
    });
  }

  /** Escape closes the field; in the whole row, so does the focus leaving it empty. */
  function onkeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && !isComposing(event)) {
      event.preventDefault();
      close();
    }
  }

  function onblur(): void {
    if (!row || search !== '') return;
    // Not when the window lost the focus: coming back finds the field where it was.
    if (!document.hasFocus()) return;
    openIn = undefined;
  }

  const BAR = 'absolute inset-0 z-10 flex items-center gap-1 bg-background px-1.5';
</script>

{#if !(row && shown)}
  <IconButton
    bind:ref={trigger}
    label={m['ViewHeader.search-text']()}
    data-tool="search"
    inert={bar}
    onclick={open}
  >
    <SearchIcon />
  </IconButton>
{/if}

<div
  role="search"
  data-testid="view-search"
  class={!shown ? 'hidden' : row ? 'w-48 shrink-0' : BAR}
>
  {#if !row}
    <IconButton label={m.viewheader_search_close()} data-tool="search-close" onclick={close}>
      <ArrowLeftIcon />
    </IconButton>
  {/if}
  <div class="relative min-w-0 flex-1">
    <SearchIcon
      class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
      aria-hidden="true"
    />
    <Input
      bind:ref={field}
      bind:value={search}
      type="search"
      aria-label={m['ViewHeader.search-text']()}
      placeholder={m['ViewHeader.search-text']()}
      autocomplete="off"
      enterkeyhint="search"
      class="pl-8 roomy:h-touch roomy:text-base"
      {onkeydown}
      {onblur}
    />
  </div>
</div>
