<!--
  What stands before a star's title, in the sidebar and in the search: a
  board's, a wiki's or a course's own (`ContainerIcon`); a card's, a page's or
  a unit's icon where it has one, else the mark of its kind.
-->
<script lang="ts">
  import FileTextIcon from '@lucide/svelte/icons/file-text';
  import GraduationCapIcon from '@lucide/svelte/icons/graduation-cap';
  import StickyNoteIcon from '@lucide/svelte/icons/sticky-note';

  import type { ContainerKind } from '$lib/logic/wire.ts';
  import type { FavoriteKind } from '$lib/state/index.ts';

  import ContainerIcon from './ContainerIcon.svelte';

  interface Props {
    kind: FavoriteKind;
    icon: string;
    title: string;
    containerKind: ContainerKind;
    locale: string;
  }

  let { kind, icon, title, containerKind, locale }: Props = $props();

  const KindIcon = $derived(
    kind === 'page' ? FileTextIcon : kind === 'unit' ? GraduationCapIcon : StickyNoteIcon,
  );
</script>

{#if kind === 'container'}
  <ContainerIcon {icon} {title} {locale} kind={containerKind} />
{:else if icon}
  <span class="flex size-4 shrink-0 items-center justify-center leading-none" aria-hidden="true">
    {icon}
  </span>
{:else}
  <KindIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
{/if}
