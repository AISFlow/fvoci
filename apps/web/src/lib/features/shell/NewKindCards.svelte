<!--
  The kinds a new thing can be, one card each with what it is for: pressed,
  that kind is made at once with the title and visibility asked above.
-->
<script lang="ts">
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import GraduationCapIcon from '@lucide/svelte/icons/graduation-cap';
  import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
  import NotebookPenIcon from '@lucide/svelte/icons/notebook-pen';
  import type { Component } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { NewKind } from '$lib/state/index.ts';

  interface Props {
    kinds: readonly NewKind[];
    onPick: (kind: NewKind) => void;
  }

  let { kinds, onPick }: Props = $props();

  const CARDS: Record<
    NewKind,
    { icon: Component<{ class?: string }>; name: () => string; hint: () => string; testId: string }
  > = {
    board: {
      icon: LayoutGridIcon,
      name: m.new_kind_board,
      hint: m.new_kind_board_hint,
      testId: 'empty-board',
    },
    wiki: {
      icon: BookOpenIcon,
      name: m.new_kind_wiki,
      hint: m.new_kind_wiki_hint,
      testId: 'new-wiki',
    },
    course: {
      icon: GraduationCapIcon,
      name: m.new_kind_course,
      hint: m.new_kind_course_hint,
      testId: 'new-course',
    },
    diary: {
      icon: NotebookPenIcon,
      name: m.new_kind_diary,
      hint: m.new_kind_diary_hint,
      testId: 'new-diary',
    },
  };
</script>

<ul class="grid grid-cols-2 gap-2" aria-label={m.new_kinds()}>
  {#each kinds as kind (kind)}
    {@const card = CARDS[kind]}
    {@const Icon = card.icon}
    <li class="contents">
      <button
        type="button"
        data-testid={card.testId}
        class="flex min-h-touch flex-col items-start gap-1 rounded-lg border bg-card p-3 text-left hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-none"
        onclick={() => {
          onPick(kind);
        }}
      >
        <span class="flex items-center gap-2 font-medium">
          <Icon class="size-4 text-muted-foreground" />
          {card.name()}
        </span>
        <span class="text-xs text-muted-foreground">{card.hint()}</span>
      </button>
    </li>
  {/each}
</ul>
