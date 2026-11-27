<!--
  One board or wiki in the sidebar: a link to it, and next to the link the
  button of its menu.

  The link itself is what is dragged (`data-drag-item`): a press on a control
  inside an item is the control's own, so the link cannot sit inside the item,
  and the menu button stands beside the link, not in it. A plain click opens
  the board in place; a click with a modifier, or the middle button, is left
  to the browser (a new tab).

  The menu button shows when the row is pointed at or holds the focus; on a
  device that cannot point without pressing it is always there.
-->
<script lang="ts">
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import type { ContainerKind } from '$lib/logic/wire.ts';

  import ContainerIcon from './ContainerIcon.svelte';
  import { menuNamed, namedAs } from './shown-title.ts';

  interface Props {
    containerId: string;
    /** The title as shown: the board's own, or the words for a board without one. */
    title: string;
    icon: string | undefined;
    /** A board when left out; a wiki or a diary says so to a screen reader too. */
    kind?: ContainerKind | undefined;
    href: string;
    /** This is the board on the page. */
    current: boolean;
    locale: string;
    /** The board can be picked up now. Not while the list is narrowed by a search. */
    draggable: boolean;
    /** On its way: kept in the document, out of sight. */
    lifted: boolean;
    /** The element that explains dragging, for a screen reader. */
    describedBy: string;
    actions: readonly MenuAction[];
    onOpen: () => void;
  }

  let {
    containerId,
    title,
    icon,
    kind,
    href,
    current,
    locale,
    draggable,
    lifted,
    describedBy,
    actions,
    onOpen,
  }: Props = $props();

  function onclick(event: MouseEvent): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpen();
  }
</script>

<li class={['group/board relative', lifted && 'hidden']} data-board={containerId}>
  <a
    {href}
    draggable="false"
    data-drag-item={draggable ? containerId : undefined}
    aria-current={current ? 'page' : undefined}
    aria-label={namedAs(kind, title)}
    aria-describedby={draggable ? describedBy : undefined}
    class="flex min-h-8 items-center gap-2 rounded-md py-1 pr-9 pl-2 select-none [-webkit-touch-callout:none] hover:bg-sidebar-accent aria-[current=page]:bg-sidebar-accent aria-[current=page]:font-medium roomy:min-h-touch roomy:pr-12"
    {onclick}
  >
    <ContainerIcon {icon} {kind} {title} {locale} />
    <span class="min-w-0 flex-1 truncate">{title}</span>
  </a>
  <!-- A guest's wiki has nothing in its menu (a board's has its export): no button then. -->
  {#if actions.length > 0}
    <ActionMenu {title} {actions}>
      {#snippet trigger(props: Record<string, unknown>)}
        <IconButton
          {...props}
          label={menuNamed(kind, title)}
          class="absolute top-0 right-0 text-muted-foreground can-hover:opacity-0 can-hover:group-focus-within/board:opacity-100 can-hover:group-hover/board:opacity-100 can-hover:aria-expanded:opacity-100"
        >
          <EllipsisIcon />
        </IconButton>
      {/snippet}
    </ActionMenu>
  {/if}
</li>
