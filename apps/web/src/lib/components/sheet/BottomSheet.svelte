<!--
  A sheet that rises from the bottom edge: what a menu or a picker becomes on
  a compact page.

  - Never taller than the screen less a strip at the top, so that what is
    behind it stays recognisable and there is somewhere to tap to dismiss it.
  - Its content scrolls inside; the title stays put.
  - Sits on top of the on-screen keyboard (`--keyboard`) and clear of the home
    indicator and of the device's left and right edges (each its own value).
  - Opening it moves the focus to the sheet itself, not to its first field: a
    focused field would bring up the keyboard before the person has seen the
    choices.
  - Closes with "Done", a tap outside, Escape, and the back button. Closed, the
    focus goes where `returnFocus` says (what opened it: `Opener`), else back to
    what had it before; then `onClosed` runs.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import * as Sheet from '$lib/ui/sheet/index.js';

  interface Props {
    open: boolean;
    title: string;
    /** Under the title: what the sheet is about, such as the name of the card. */
    subtitle?: string;
    /** Fills the height it is allowed even when its content is shorter. */
    tall?: boolean;
    onOpenChange?: (open: boolean) => void;
    /** Where the focus goes once the sheet has closed: what opened it. */
    returnFocus?: () => HTMLElement | null | undefined;
    /** Once the sheet has closed and given the focus back. */
    onClosed?: () => void;
    children: Snippet;
  }

  let {
    open = $bindable(),
    title,
    subtitle,
    tall = false,
    onOpenChange,
    returnFocus,
    onClosed,
    children,
  }: Props = $props();

  let content = $state<HTMLElement | null>(null);

  closeOnBack(
    () => open,
    () => {
      open = false;
      onOpenChange?.(false);
    },
  );
</script>

<Sheet.Root bind:open {onOpenChange}>
  <Sheet.Content
    bind:ref={content}
    side="bottom"
    showCloseButton={false}
    data-testid="bottom-sheet"
    class={[
      'max-h-[calc(100dvh-var(--safe-top)-var(--keyboard)-3rem)] gap-0 rounded-t-2xl pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) outline-none data-[side=bottom]:bottom-(--keyboard)',
      tall && 'data-[side=bottom]:h-dvh',
    ]}
    onOpenAutoFocus={(event: Event) => {
      event.preventDefault();
      content?.focus();
    }}
    onCloseAutoFocus={(event: Event) => {
      const back = returnFocus?.();
      if (back?.isConnected) {
        event.preventDefault();
        back.focus({ preventScroll: true });
      }
      onClosed?.();
    }}
  >
    <Sheet.Header class="shrink-0 flex-row items-center gap-2 py-2 pr-2 pl-4">
      <div class="min-w-0 flex-1">
        <Sheet.Title class="truncate pr-0">{title}</Sheet.Title>
        {#if subtitle}
          <Sheet.Description class="truncate text-xs text-muted-foreground">
            {subtitle}
          </Sheet.Description>
        {/if}
      </div>
      <Sheet.Close
        class="h-8 shrink-0 rounded-lg px-3 text-sm font-medium text-foreground hover:bg-muted roomy:h-touch"
      >
        {m.ui_done()}
      </Sheet.Close>
    </Sheet.Header>
    <div
      data-presentation="sheet"
      class="group/surface min-h-0 flex-1 overflow-y-auto overscroll-contain"
    >
      {@render children()}
    </div>
  </Sheet.Content>
</Sheet.Root>
