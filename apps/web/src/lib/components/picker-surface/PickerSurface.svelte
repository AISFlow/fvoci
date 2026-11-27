<!--
  The surface a list of choices or a date picker opens on: a popover next to
  its trigger on a wide screen, a sheet from the bottom edge on a narrow one.

  Which one is used is the `presentation` argument; the caller decides
  (default-presentation.svelte.ts has the default, which follows the stage of
  the page by the rule of lib/layout/presentation.ts). The sheet is the same
  one menus use (../sheet/BottomSheet.svelte): it closes with the back button
  too, and sits on top of the on-screen keyboard.

  Inside the surface, `group-data-[presentation=sheet]/surface:` styles a
  child for the sheet only: rows a finger has to hit grow to `min-h-touch`.

  Where the focus goes on opening differs on purpose. In the popover it goes
  to the element marked `data-picker-focus` (the field of a list), or else to
  the first control; under a finger it goes to the popover itself. On the sheet it goes to the sheet itself: focusing a
  field there would raise the on-screen keyboard over half of the choices
  before the person has asked to type.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import { Opener } from '$lib/focus.ts';
  import { fingerPointer, insets } from '$lib/layout/room.svelte.ts';
  import * as Popover from '$lib/ui/popover/index.js';
  import { cn } from '$lib/utils.js';

  import BottomSheet from '../sheet/BottomSheet.svelte';

  import type { Presentation } from './presentation.ts';

  interface Props {
    open?: boolean;
    presentation: Presentation;
    /** Names the surface for assistive technology; the heading of the sheet. */
    title: string;
    /** The element that opens the surface. Spread `props` on a button, before its own attributes. */
    trigger: Snippet<[{ props: Record<string, unknown> }]>;
    children: Snippet;
    onOpenChange?: (open: boolean) => void;
    /** Where the focus goes once the surface has closed, in place of a trigger nobody can focus. */
    returnFocus?: () => HTMLElement | null | undefined;
    class?: string;
  }

  let {
    open = $bindable(false),
    presentation,
    title,
    trigger,
    children,
    onOpenChange,
    returnFocus,
    class: className,
  }: Props = $props();

  let popover = $state<HTMLElement | null>(null);
  const opener = new Opener();

  function focusMarked(event: Event): void {
    // Under a finger the focus stays out of the field: the keyboard comes up
    // when the person taps it, not before they have seen the choices.
    if (fingerPointer.current) {
      event.preventDefault();
      popover?.focus();
      return;
    }
    const marked = popover?.querySelector<HTMLElement>('[data-picker-focus]');
    if (!marked) return;
    event.preventDefault();
    marked.focus();
  }
</script>

{#if presentation === 'sheet'}
  {@render trigger({
    props: {
      'aria-haspopup': 'dialog',
      'aria-expanded': open,
      // In the capture phase: a trigger may put its own `onclick` in place of this one's.
      onclickcapture: (event: MouseEvent) => {
        opener.remember(event);
      },
      onclick: () => {
        open = true;
        onOpenChange?.(true);
      },
    },
  })}
  <BottomSheet
    bind:open
    {title}
    {onOpenChange}
    returnFocus={() => returnFocus?.() ?? opener.take()}
  >
    <div class={cn('px-2 pb-2', className)}>
      {@render children()}
    </div>
  </BottomSheet>
{:else}
  <Popover.Root bind:open {onOpenChange}>
    <Popover.Trigger>
      {#snippet child({ props }: { props: Record<string, unknown> })}
        {@render trigger({ props })}
      {/snippet}
    </Popover.Trigger>
    <Popover.Content
      bind:ref={popover}
      align="start"
      collisionPadding={insets.collisionPadding}
      aria-label={title}
      class="max-h-(--bits-popover-content-available-height) w-auto max-w-[calc(100vw-1rem-var(--safe-left)-var(--safe-right))] min-w-64 gap-0 overflow-y-auto p-1"
      onOpenAutoFocus={focusMarked}
      onCloseAutoFocus={(event: Event) => {
        const back = returnFocus?.();
        if (!back?.isConnected) return;
        event.preventDefault();
        back.focus({ preventScroll: true });
      }}
    >
      <div data-presentation="popover" class={cn('group/surface', className)}>
        {@render children()}
      </div>
    </Popover.Content>
  </Popover.Root>
{/if}
