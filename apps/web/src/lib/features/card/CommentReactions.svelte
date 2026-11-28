<!--
  The reactions to a comment, a card, a page or a unit: each emoji with how
  many answered with it, the reader's own marked (and pressed: `aria-pressed`).
  Its name (and the tooltip of a mouse) says who: "you", the people the
  reader may see by name, and "someone" for the rest (a guest is not told the
  names of whom they do not share a board with).

  Who may comment presses one to add theirs or take it off, and adds another
  with the button at the end: the emoji most answered with first, then any
  by its name (`lib/editor/emoji.ts`). It opens the way every picker does: a
  popover, or a sheet on a compact page, which a long press on a comment
  opens too (`open`). A block takes 20 different emoji: past that only those
  already there are offered. Who may not comment sees them and presses none.
-->
<script lang="ts">
  import SmilePlusIcon from '@lucide/svelte/icons/smile-plus';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { loadEmoji, searchEmoji, type EmojiIndex } from '$lib/editor/emoji.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ThreadReactions } from '$lib/state/comment-thread.svelte.ts';
  import {
    QUICK_REACTIONS,
    REACTION_KINDS_PER_BLOCK,
    hasReacted,
    type Reaction,
  } from '$lib/state/reaction-types.ts';

  interface Props {
    /** The reactions of the block and its comments, and the reader's toggles. */
    reactions: ThreadReactions;
    /** The comment, card, page or unit. */
    blockId: string;
    /** Names the button that adds one: what it answers. */
    addLabel?: string;
    /** Whether the emoji to add are open. */
    open?: boolean;
    class?: string;
  }

  let {
    reactions,
    blockId,
    addLabel = m.reaction_add(),
    open = $bindable(false),
    class: className,
  }: Props = $props();

  const presentation = new PickerPresentation();
  const shown = $derived(reactions.reactionsOf(blockId));
  const workspace = $derived(reactions.workspace);
  const me = $derived(workspace.me?.id);
  const full = $derived(shown.length >= REACTION_KINDS_PER_BLOCK);
  const names = $derived(new Intl.ListFormat(workspace.locale, { type: 'conjunction' }));

  let search = $state('');
  let index = $state.raw<EmojiIndex | undefined>();
  const found = $derived(index && search.trim() !== '' ? searchEmoji(index, search, 40) : []);

  /** Who answered with it, by name: the reader as "you", whom the reader is not told of as "someone". */
  function who(reaction: Reaction): string {
    return names.format(
      reaction.userIds.map((userId) =>
        userId === me ? m.reaction_you() : (workspace.userName(userId) ?? m.reaction_someone()),
      ),
    );
  }

  function labelOf(reaction: Reaction): string {
    const said = { emoji: reaction.emoji, count: reaction.userIds.length, names: who(reaction) };
    return hasReacted(shown, reaction.emoji, me)
      ? m.reaction_chip_mine(said)
      : m.reaction_chip(said);
  }

  /** Whether the emoji can be pressed: one already there always, a new one while there is room. */
  const offered = (emoji: string): boolean =>
    !full || shown.some((reaction) => reaction.emoji === emoji);

  function toggle(emoji: string): void {
    void reactions.toggleReaction(blockId, emoji);
  }

  function pick(emoji: string): void {
    open = false;
    search = '';
    toggle(emoji);
  }

  function readIndex(): void {
    if (index) return;
    void loadEmoji(workspace.locale).then((loaded) => {
      index = loaded;
    });
  }
</script>

{#if shown.length > 0 || reactions.canReact}
  <div
    role="group"
    aria-label={m.reaction_group()}
    class={['flex flex-wrap items-center gap-1.5', className]}
    data-testid="reactions"
    data-block={blockId}
  >
    {#each shown as reaction (reaction.emoji)}
      {@const mine = hasReacted(shown, reaction.emoji, me)}
      {@const label = labelOf(reaction)}
      {@const chip = [
        'inline-flex h-7 min-w-11 items-center justify-center gap-1 rounded-full border px-2 text-sm tabular-nums roomy:min-h-touch roomy:min-w-touch',
        mine ? 'border-primary bg-primary/10 font-medium' : 'border-border bg-muted/40',
      ]}
      {#if reactions.canReact}
        <button
          type="button"
          class={[
            chip,
            'hover:bg-accent focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none',
          ]}
          aria-pressed={mine}
          aria-label={label}
          title={label}
          data-reaction={reaction.emoji}
          onclick={() => {
            toggle(reaction.emoji);
          }}
        >
          <span aria-hidden="true">{reaction.emoji}</span>
          <span aria-hidden="true">{reaction.userIds.length}</span>
        </button>
      {:else}
        <span
          role="img"
          class={chip}
          aria-label={label}
          title={label}
          data-reaction={reaction.emoji}
        >
          <span aria-hidden="true">{reaction.emoji}</span>
          <span aria-hidden="true">{reaction.userIds.length}</span>
        </span>
      {/if}
    {/each}
    {#if reactions.canReact}
      <PickerSurface
        bind:open
        presentation={presentation.current}
        title={addLabel}
        onOpenChange={(now: boolean) => {
          if (!now) search = '';
        }}
      >
        {#snippet trigger({ props }: { props: Record<string, unknown> })}
          <IconButton
            {...props}
            label={addLabel}
            class="h-7 w-9 rounded-full text-muted-foreground"
            data-action="react"
          >
            <SmilePlusIcon aria-hidden="true" />
          </IconButton>
        {/snippet}
        <div
          class="flex w-72 max-w-full flex-col gap-2 p-1 roomy:w-80"
          data-testid="reaction-picker"
        >
          <div role="group" aria-label={m.reaction_quick()} class="flex justify-between gap-1">
            {#each QUICK_REACTIONS as emoji (emoji)}
              <button
                type="button"
                class="grid size-10 place-items-center rounded-md text-xl hover:bg-accent disabled:opacity-40 aria-pressed:bg-primary/10 roomy:size-touch"
                aria-pressed={hasReacted(shown, emoji, me)}
                aria-label={emoji}
                disabled={!offered(emoji)}
                data-quick={emoji}
                onclick={() => {
                  pick(emoji);
                }}
              >
                {emoji}
              </button>
            {/each}
          </div>
          {#if full}
            <p class="text-xs text-muted-foreground" role="status">{m.reaction_full()}</p>
          {/if}
          <input
            bind:value={search}
            type="search"
            data-picker-focus
            aria-label={m.reaction_search()}
            placeholder={m.reaction_search()}
            class="h-8 rounded-lg border border-input-border bg-transparent px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30 roomy:min-h-touch"
            onfocus={readIndex}
            oninput={readIndex}
          />
          {#if search.trim() !== '' && index}
            {#if found.length === 0}
              <p class="px-1 text-sm text-muted-foreground">{m.reaction_none_found()}</p>
            {:else}
              <div class="grid grid-cols-8 gap-0.5 roomy:grid-cols-6">
                {#each found as match (match.emoji)}
                  <button
                    type="button"
                    class="grid aspect-square place-items-center rounded-md text-lg hover:bg-accent disabled:opacity-40 roomy:min-h-touch"
                    aria-label={match.name}
                    title={match.name}
                    disabled={!offered(match.emoji)}
                    onclick={() => {
                      pick(match.emoji);
                    }}
                  >
                    {match.emoji}
                  </button>
                {/each}
              </div>
            {/if}
          {/if}
        </div>
      </PickerSurface>
    {/if}
  </div>
{/if}
