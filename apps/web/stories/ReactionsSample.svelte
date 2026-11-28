<!--
  The reactions to a card of the example board and to its comments: the
  reader's own pressed, those of a member, and one of somebody the reader is
  not told of ("someone"); or as a viewer, who presses none; or a card with
  as many different emoji as a block takes (`full`).
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import CardComments from '$lib/features/card/CardComments.svelte';
  import CommentReactions from '$lib/features/card/CommentReactions.svelte';
  import type { BoardStore, CardStore } from '$lib/state/index.ts';
  import { MemoryBackend } from '$lib/state/memory/backend.ts';

  import type { DocNode } from '$lib/editor/schema.ts';

  import { exampleWorkspace } from './workspace.ts';

  let { readOnly = false, full = false }: { readOnly?: boolean; full?: boolean } = $props();

  /** A body of one paragraph saying `text`. */
  const body = (text: string): DocNode => ({
    type: 'doc',
    content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
  });

  let board = $state.raw<BoardStore | undefined>();
  let card = $state.raw<CardStore | undefined>();

  onMount(() => {
    const example = exampleWorkspace({ count: 1, readOnly });
    const { world } = example;
    const cardId = 'example-card-1';
    async function seed(): Promise<void> {
      const author = new MemoryBackend(world, 'example-member');
      const [first, second] = [
        await author.createComment('example-board', cardId, body('Looks good to me · 좋아요')),
        await author.createComment(
          'example-board',
          cardId,
          body('Shipping it today · 오늘 내보냅니다'),
        ),
      ];
      const at = [world.workspace.id, 'example-board'] as const;
      await author.reactions.add(...at, first.id, '👍');
      await author.reactions.add(...at, first.id, '🎉');
      await author.reactions.add(...at, cardId, '🚀');
      if (!readOnly) {
        await example.backend.reactions.add(...at, first.id, '👍');
        await example.backend.reactions.add(...at, second.id, '❤️');
      }
      // Somebody the reader is not told of: a guest elsewhere, say.
      world.reactions.set(second.id, [
        ...(world.reactions.get(second.id) ?? []),
        { emoji: '👀', userIds: ['someone-elsewhere'] },
      ]);
      if (full) {
        for (let at = 0; at < 19; at += 1) {
          await author.reactions.add(
            world.workspace.id,
            'example-board',
            cardId,
            String.fromCodePoint(0x1f400 + at),
          );
        }
      }
      await example.workspace.load();
      const opened = await example.workspace.openBoard('example-board');
      board = opened;
      card = await opened.showCard(cardId);
    }
    void seed();
    return () => {
      example.workspace.close();
    };
  });
</script>

<div class="flex max-w-xl flex-col gap-4 p-4" data-testid="reactions-example">
  {#if board && card?.status === 'ready'}
    <CommentReactions reactions={card.reactions} blockId={card.cardId} />
    <CardComments
      {board}
      {card}
      reactions={card.reactions}
      readVersions={() => Promise.resolve([])}
    />
  {/if}
</div>
