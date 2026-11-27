<!--
  Editor for one Gantt dependency arrow: lets a reader who may edit choose the
  dependency kind (`FS`, `SS`, `FF`, `SF`), adjust `lagDays` (`-3650..=3650`)
  with step buttons or direct input, save in one atomic card-property write
  (preserving every other link and hidden link on the target card), or remove
  the dependency.

  Opens as a Popover anchored to the arrow control on desktop, or a BottomSheet
  on compact screens.
-->
<script lang="ts">
  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import MinusIcon from '@lucide/svelte/icons/minus';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import { relationKindName } from '$lib/components/properties/relation-labels.ts';
  import BottomSheet from '$lib/components/sheet/BottomSheet.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { insets } from '$lib/layout/room.svelte.ts';
  import {
    clampRelationLagDays,
    parseLagDaysInput,
    parseRelationValue,
    RELATION_KINDS,
    RELATION_LAG_MAX,
    RELATION_LAG_MIN,
    removeRelationLink,
    updateRelationLink,
    type RelationKind,
  } from '$lib/logic/relations.ts';
  import { cardProperty } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import * as Popover from '$lib/ui/popover/index.js';

  import type { ArrowLine } from './chart.svelte.ts';

  interface Props {
    arrow: ArrowLine | undefined;
    open: boolean;
    anchor: HTMLElement | null;
    board: BoardStore;
    dependencyPropertyId: string | undefined;
    sheet: boolean;
    onclosed: (removed: boolean, toCardId: string | undefined, fromCardId?: string) => void;
  }

  let {
    arrow = $bindable(),
    open = $bindable(),
    anchor,
    board,
    dependencyPropertyId,
    sheet,
    onclosed,
  }: Props = $props();

  const uid = $props.id();
  let box = $state.raw<HTMLElement | null>(null);
  let announced = $state('');
  let writing = $state(0);
  let removedLast = false;

  let draftKind = $derived<RelationKind>(arrow?.kind ?? 'fs');
  let draftLagText = $derived(String(arrow?.lagDays ?? 0));

  const parsedLag = $derived(parseLagDaysInput(draftLagText));
  const lagError = $derived(parsedLag === undefined);
  const changed = $derived(
    arrow !== undefined &&
      parsedLag !== undefined &&
      (draftKind !== arrow.kind || parsedLag !== arrow.lagDays),
  );

  function stepLag(delta: number): void {
    const base = parsedLag ?? arrow?.lagDays ?? 0;
    draftLagText = String(clampRelationLagDays(base + delta));
  }

  function currentLinks(toCardId: string) {
    if (!dependencyPropertyId) return undefined;
    const card = board.cards.find((each) => each.id === toCardId);
    if (!card) return undefined;
    return parseRelationValue(cardProperty(card, dependencyPropertyId));
  }

  function radioKeys(event: KeyboardEvent, index: number): void {
    const delta =
      event.key === 'ArrowRight' || event.key === 'ArrowDown'
        ? 1
        : event.key === 'ArrowLeft' || event.key === 'ArrowUp'
          ? -1
          : 0;
    if (delta === 0) return;
    event.preventDefault();
    const next = RELATION_KINDS[(index + delta + RELATION_KINDS.length) % RELATION_KINDS.length];
    if (!next) return;
    draftKind = next;
    const container = (event.currentTarget as HTMLElement).closest('[role="radiogroup"]');
    container?.querySelector<HTMLElement>(`[data-relation-kind="${next}"]`)?.focus();
  }

  async function save(): Promise<void> {
    if (!arrow || !dependencyPropertyId || parsedLag === undefined) return;
    const target = arrow;
    const nextKind = draftKind;
    const nextLag = parsedLag;
    const links = currentLinks(target.to);
    if (!links?.some((link) => link.card === target.from)) {
      removedLast = true;
      open = false;
      if (sheet) closed();
      return;
    }
    removedLast = false;
    open = false;
    if (sheet) closed();
    if (nextKind === target.kind && nextLag === target.lagDays) return;
    announced = '';
    writing += 1;
    try {
      const updated = updateRelationLink(links, target.from, { kind: nextKind, lagDays: nextLag });
      await board.setCardProperty(target.to, dependencyPropertyId, updated);
      const saved = currentLinks(target.to)?.find((link) => link.card === target.from);
      if (saved?.kind === nextKind && saved.lagDays === nextLag) {
        announced = m.gantt_relation_saved({ from: target.fromTitle, to: target.toTitle });
      }
    } finally {
      writing -= 1;
    }
  }

  async function remove(): Promise<void> {
    if (!arrow || !dependencyPropertyId) return;
    const target = arrow;
    const links = currentLinks(target.to);
    if (!links?.some((link) => link.card === target.from)) {
      removedLast = true;
      open = false;
      if (sheet) closed();
      return;
    }
    removedLast = true;
    open = false;
    if (sheet) closed();
    announced = '';
    writing += 1;
    try {
      const updated = removeRelationLink(links, target.from);
      await board.setCardProperty(target.to, dependencyPropertyId, updated);
      const stillPresent = currentLinks(target.to)?.some((link) => link.card === target.from);
      if (!stillPresent) {
        announced = m.gantt_relation_removed({ from: target.fromTitle, to: target.toTitle });
      }
    } finally {
      writing -= 1;
    }
  }

  function closed(): void {
    if (arrow === undefined) return;
    const toCardId = arrow.to;
    const fromCardId = arrow.from;
    const removed = removedLast;
    removedLast = false;
    open = false;
    arrow = undefined;
    onclosed(removed, toCardId, fromCardId);
  }

  function focusSelectedKind(event: Event): void {
    event.preventDefault();
    (
      box?.querySelector<HTMLElement>(`[data-relation-kind="${draftKind}"]`) ??
      box?.querySelector<HTMLElement>('[data-relation-kind]')
    )?.focus();
  }
</script>

<p
  class="sr-only"
  role="status"
  data-testid="gantt-relation-status"
  data-settling={writing > 0 ? '' : undefined}
>
  {announced}
</p>

{#snippet kindPicker()}
  <div class="flex flex-col gap-1.5">
    <span id="{uid}-kind-label" class="text-xs font-medium text-muted-foreground">
      {m.gantt_relation_kind()}
    </span>
    <div role="radiogroup" aria-labelledby="{uid}-kind-label" class="grid grid-cols-2 gap-1">
      {#each RELATION_KINDS as kind, index (kind)}
        {@const selected = draftKind === kind}
        <button
          type="button"
          role="radio"
          aria-checked={selected}
          tabindex={selected ? 0 : -1}
          data-relation-kind={kind}
          class={[
            'flex flex-col items-start rounded-md border px-2 py-1.5 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch',
            selected
              ? 'border-primary bg-primary/10 text-foreground'
              : 'border-border bg-background text-muted-foreground hover:bg-muted hover:text-foreground',
          ]}
          onclick={() => {
            draftKind = kind;
          }}
          onkeydown={(event) => {
            radioKeys(event, index);
          }}
        >
          <span class="text-xs font-semibold">{kind.toUpperCase()}</span>
          <span class="text-[0.6875rem] leading-tight">{relationKindName(kind)}</span>
        </button>
      {/each}
    </div>
  </div>
{/snippet}

{#snippet lagControl()}
  <div class="flex flex-col gap-1.5">
    <label for="{uid}-lag" class="text-xs font-medium text-muted-foreground">
      {m.gantt_relation_lag()}
    </label>
    <div class="flex items-center gap-1">
      <Button
        type="button"
        variant="outline"
        size="icon-sm"
        data-testid="relation-lag-dec"
        aria-label={m.gantt_relation_lag_decrease()}
        disabled={(parsedLag ?? 0) <= RELATION_LAG_MIN}
        onclick={() => {
          stepLag(-1);
        }}
      >
        <MinusIcon class="size-3.5" aria-hidden="true" />
      </Button>
      <Input
        id="{uid}-lag"
        data-testid="relation-lag-input"
        type="text"
        aria-invalid={lagError ? 'true' : undefined}
        aria-describedby={lagError ? `${uid}-lag-error` : undefined}
        class="h-8 flex-1 text-center tabular-nums"
        bind:value={draftLagText}
      />
      <Button
        type="button"
        variant="outline"
        size="icon-sm"
        data-testid="relation-lag-inc"
        aria-label={m.gantt_relation_lag_increase()}
        disabled={(parsedLag ?? 0) >= RELATION_LAG_MAX}
        onclick={() => {
          stepLag(1);
        }}
      >
        <PlusIcon class="size-3.5" aria-hidden="true" />
      </Button>
    </div>
    {#if lagError}
      <p id="{uid}-lag-error" role="alert" class="text-xs text-destructive">
        {m.gantt_relation_lag_range({ min: RELATION_LAG_MIN, max: RELATION_LAG_MAX })}
      </p>
    {/if}
  </div>
{/snippet}

{#snippet editorBody()}
  {#if arrow}
    <form
      data-testid="gantt-relation-editor"
      class="flex w-72 max-w-full flex-col gap-3 p-3"
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
        <span class="min-w-0 truncate font-medium text-foreground">{arrow.fromTitle}</span>
        <span class="sr-only"> → </span>
        <ArrowRightIcon class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="min-w-0 truncate font-medium text-foreground">{arrow.toTitle}</span>
      </div>
      {@render kindPicker()}
      {@render lagControl()}
      <div class="flex items-center justify-between gap-2 pt-1">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          data-testid="relation-remove"
          class="gap-1 text-destructive hover:bg-destructive/10 hover:text-destructive"
          onclick={() => void remove()}
        >
          <Trash2Icon class="size-3.5" aria-hidden="true" />
          <span>{m.gantt_relation_remove()}</span>
        </Button>
        <Button type="submit" size="sm" data-testid="relation-save" disabled={lagError || !changed}>
          {m.gantt_relation_save()}
        </Button>
      </div>
    </form>
  {/if}
{/snippet}

{#if arrow !== undefined}
  {#if sheet}
    <BottomSheet
      bind:open
      title={m.gantt_relation_edit_title()}
      subtitle={`${arrow.fromTitle} → ${arrow.toTitle}`}
      returnFocus={() => (removedLast ? null : anchor)}
      onOpenChange={(next: boolean) => {
        if (!next) closed();
      }}
    >
      {@render editorBody()}
    </BottomSheet>
  {:else}
    <Popover.Root bind:open>
      <Popover.Content
        bind:ref={box}
        customAnchor={anchor ?? undefined}
        side="bottom"
        align="center"
        collisionPadding={insets.collisionPadding}
        aria-label={m.gantt_relation_edit_title()}
        class="max-h-[calc(100dvh-1rem)] w-auto max-w-[calc(100vw-1rem-var(--safe-left)-var(--safe-right))] overflow-y-auto p-0"
        onOpenAutoFocus={focusSelectedKind}
        onCloseAutoFocus={(event: Event) => {
          event.preventDefault();
          closed();
        }}
      >
        {@render editorBody()}
      </Popover.Content>
    </Popover.Root>
  {/if}
{/if}
