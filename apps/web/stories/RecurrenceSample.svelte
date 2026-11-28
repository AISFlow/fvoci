<!--
  How a card repeats, as its due date's picker says it: the stand keeps the
  recurrence, applies what the fields report and writes it on the screen.
  Where the board does not let cards repeat, its settings that do come
  under it. The face of a card that repeats, and the notice of a next card,
  are drawn beside it.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import BoardDueRules from '$lib/components/board-rules/BoardDueRules.svelte';
  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import RecurrenceFields from '$lib/components/recurrence/RecurrenceFields.svelte';
  import NextCardToast from '$lib/features/card/NextCardToast.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import type { Recurrence } from '$lib/logic/recurrence.ts';
  import type { DoneRule, PropertyTemplate } from '$lib/logic/wire.ts';
  import { nextCardNotices } from '$lib/state/index.ts';

  import { exampleCard, STORY_NOW } from './fixtures.ts';
  import { exampleWorkspace } from './workspace.ts';

  interface Props {
    /** How the card repeats at first. */
    recurrence?: Recurrence;
    /** The board has a rule of done and names its due date. */
    allowed?: boolean;
    /** A notice of the next card #12 is shown. */
    notice?: boolean;
  }

  let { recurrence: initial, allowed = true, notice = false }: Props = $props();

  // The story's starting value, then what the fields report.
  let recurrence = $state(untrack(() => initial));
  let dueDatePropertyId = $state<string | undefined>(undefined);
  let doneWhen = $state<DoneRule | undefined>(undefined);
  const PROPERTIES: PropertyTemplate[] = [
    { id: 'due', name: 'Due · 마감', type: 'date', options: [] },
    {
      id: 'status',
      name: 'Status · 상태',
      type: 'select',
      options: [
        { id: 'todo', label: 'To do · 할 일', color: 'blue' },
        { id: 'done', label: 'Done · 완료', color: 'green' },
      ],
    },
  ];
  const regional = { timeZone: 'Asia/Seoul', weekStart: 1, hourCycle: 'h23' } as const;
  const card = $derived.by(() => {
    const made = exampleCard(1);
    made.fields.properties.due = { from: '2026-11-20' };
    if (recurrence) made.fields.recurrence = recurrence;
    return made;
  });

  const example = exampleWorkspace({ kind: 'board', count: 1 });
  if (untrack(() => notice)) {
    nextCardNotices(example.workspace).notice = { id: 1, number: 12, count: 1, checkpoint: 0 };
  }
</script>

<div class="flex max-w-sm flex-col gap-4 p-2" data-testid="recurrence-example">
  <div class="rounded-lg border bg-popover p-1" data-presentation="popover">
    <RecurrenceFields
      {recurrence}
      {allowed}
      onChange={(next: Recurrence | undefined) => {
        recurrence = next;
      }}
    />
    {#if !allowed}
      <BoardDueRules
        properties={PROPERTIES}
        {dueDatePropertyId}
        {doneWhen}
        onDueProperty={(propertyId: string | undefined) => {
          dueDatePropertyId = propertyId;
        }}
        onDoneRule={(rule: DoneRule | undefined) => {
          doneWhen = rule;
        }}
      />
    {/if}
  </div>
  <output class="text-xs text-muted-foreground" data-testid="stored-recurrence">
    {JSON.stringify(recurrence ?? null)}
  </output>
  <div class="w-64 rounded-lg border p-3">
    <CardFace
      {card}
      templates={PROPERTIES}
      shownProperties={['due']}
      locale={getLocale()}
      {regional}
      now={STORY_NOW}
    />
  </div>
  <NextCardToast workspace={example.workspace} />
</div>
