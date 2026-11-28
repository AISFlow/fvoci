<!--
  The activity of a card of the example board, folded at first: its making
  by a member, a change of its title, an agent's change of its status, and a
  change by someone the reader may not see, who is shown without a name.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CardActivity from '$lib/features/card/CardActivity.svelte';
  import type { CardActivityLine } from '$lib/state/index.ts';

  import { STORY_NOW } from './fixtures.ts';
  import { exampleWorkspace, loadWorkspace } from './workspace.ts';

  interface Props {
    /** The width of an iPhone: 390px. */
    narrow?: boolean;
  }

  let { narrow = false }: Props = $props();

  const CARD = 'example-card-1';
  const MINUTE = 60_000;
  const line = (
    id: string,
    minutesAgo: number,
    rest: Partial<CardActivityLine> & Pick<CardActivityLine, 'kind'>,
  ): CardActivityLine => ({
    id,
    propertyId: null,
    before: null,
    after: null,
    actorId: 'example-member',
    actorTokenName: null,
    createdAt: STORY_NOW - minutesAgo * MINUTE,
    updatedAt: STORY_NOW - minutesAgo * MINUTE,
    ...rest,
  });

  const example = exampleWorkspace({ kind: 'board', count: 3 });
  example.world.cardEvents.set(CARD, [
    line('made', 120, { kind: 'created' }),
    line('renamed', 90, {
      kind: 'title',
      before: 'Example card',
      after: 'Example card 001 · 예시 카드',
      actorId: 'example-reader',
    }),
    line('agent', 30, {
      kind: 'property',
      propertyId: 'status',
      before: 'todo',
      after: 'done',
      actorTokenName: 'Weekly digest · 주간 정리 에이전트, a long name of a token',
    }),
    line('unseen', 5, {
      kind: 'property',
      propertyId: 'due',
      before: { from: '2026-11-04', to: '2026-11-06' },
      after: { from: '2026-11-11' },
      actorId: 'someone-the-reader-cannot-see',
    }),
  ]);
  let ready = $state(false);
  const board = $derived(ready ? example.workspace.board : undefined);
  const card = $derived(board?.card(CARD));
</script>

<div
  class="flex flex-col p-2"
  style:max-width={narrow ? '390px' : '40rem'}
  data-testid="card-activity-example"
  {@attach () => {
    untrack(() => {
      void loadWorkspace(example).then(() => {
        ready = true;
      });
    });
  }}
>
  {#if board && card}
    <CardActivity {board} {card} />
  {/if}
</div>
