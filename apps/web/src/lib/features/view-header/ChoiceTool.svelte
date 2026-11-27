<!--
  A tool of the header that picks one of a short list: what the view is
  grouped by, what date it is placed by, which card relation it reads as
  what comes before what. The button is the header's own (`tool`), so that
  it folds with the rest (the last of the three has no button at all and is
  opened from "…"); the list is a menu, or a sheet on a compact page.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';

  import type { Icon } from './texts.ts';

  interface Props {
    /** Bindable. */
    open: boolean;
    /** Names the tool in the markup (`data-tool`). */
    id: string;
    icon: Icon;
    /** What the list is of, and the view it is of. */
    title: string;
    subtitle: string;
    /** What the button says: the title with the current choice. */
    word: string;
    actions: readonly MenuAction[];
    /** Whether the tool has a button: none in the folded row, or (`always`) none in either row. */
    aside?: boolean | 'always';
    /** The button of a tool as the header draws it. */
    tool: Snippet<[Record<string, unknown>, string, Icon, string, number, boolean | 'always']>;
    /** Told after the list opened or closed. */
    onchange: (open: boolean) => void;
  }

  let {
    open = $bindable(),
    id,
    icon,
    title,
    subtitle,
    word,
    actions,
    aside = true,
    tool,
    onchange,
  }: Props = $props();
</script>

<ActionMenu
  bind:open={
    () => open,
    (next: boolean) => {
      open = next;
      onchange(next);
    }
  }
  {title}
  {subtitle}
  {actions}
  align="start"
>
  {#snippet trigger(props: Record<string, unknown>)}
    {@render tool(props, id, icon, word, 0, aside)}
  {/snippet}
</ActionMenu>
