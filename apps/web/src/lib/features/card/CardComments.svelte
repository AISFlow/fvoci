<!--
  The comments of a card, oldest first: who wrote it, how long ago, and the
  text, drawn with the read-only view of a body. A comment somebody else
  writes while the card is open appears by itself (the card store follows
  the board's changes) and is announced.

  The text of a comment is changed in place (the same plain field as a new
  comment: lines are paragraphs) by whom the server lets (`canEditComment`),
  and deleted the same way; on a card, "Undo" in its menu brings a deleted one
  back. A changed comment says who changed it and when, and any member can
  open what it said before (`CommentHistory`). The field for a new comment is not here: it is pinned to the
  bottom edge of the card (CommentComposer).

  Under each its reactions (`CommentReactions`); a long press of a finger on
  a comment opens the emoji to answer it with, for who may comment.

  A wiki's page has comments of the same kind: they are shown by this too,
  with the page's comments (`PageComments`) in place of both the board and
  the card; "Undo" brings a deleted one back there too, and the heading is
  one level under the page's title.
-->
<script lang="ts">
  import HistoryIcon from '@lucide/svelte/icons/history';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import { onMount, tick } from 'svelte';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { isComposing, isSaveKey, primaryModifier, shortcutKeys } from '$lib/keys.ts';
  import BodyView from '$lib/editor/BodyView.svelte';
  import { longPress } from '$lib/editor/touch-press.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { displayDateTime, relativeTime } from '$lib/logic/format.ts';
  import { agentTokenNameOf } from '$lib/logic/wire-blocks.ts';
  import type { ThreadReactions } from '$lib/state/comment-thread.svelte.ts';
  import type { BoardStore, CardStore, CommentVersion } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { autosize } from '$lib/components/autosize.ts';
  import { bodyReferences } from './body-references.ts';
  import { commentDoc, commentText } from './comment-doc.ts';
  import CommentHistory from './CommentHistory.svelte';
  import CommentReactions from './CommentReactions.svelte';

  interface Props {
    /** The board, or the page's comments: who may do what, and who the names are. */
    board: Pick<
      BoardStore,
      'workspace' | 'canEditComment' | 'canDeleteComment' | 'canReadCommentHistory'
    >;
    /** The card, or the page's comments: the comments and their changes. */
    card: Pick<CardStore, 'comments' | 'comment' | 'editComment' | 'deleteComment'>;
    /** What a comment said before (`CommentHistory`). */
    readVersions: (commentId: string) => Promise<CommentVersion[]>;
    /** The reactions to the comments. */
    reactions: ThreadReactions;
    /** The level of the heading: under a card's title, or under a page's. */
    heading?: 'h2' | 'h3';
  }

  let { board, card, readVersions, reactions, heading = 'h3' }: Props = $props();

  /** The comment whose emoji to answer it with are open. */
  let reacting = $state<string | undefined>();

  /** Deletes the comment; its field and its history, if open, go with it. */
  function remove(commentId: string): void {
    if (editing?.id === commentId) editing = undefined;
    if (historyOf === commentId) historyOf = undefined;
    void card.deleteComment(commentId);
  }

  // The comments are drawn, not written in: what `[[` offers is none of theirs.
  const references = $derived(bodyReferences(board));

  const uid = $props.id();
  const workspace = $derived(board.workspace);
  // Somebody not known (gone, or a mention that names nobody, as an anonymous reader gets them).
  const resolveUser = (userId: string): string =>
    workspace.userName(userId) ?? m['centerPanel.unknown-user']();

  /** The current moment, for "3 minutes ago". Moves on once a minute. */
  let now = $state(Date.now());
  onMount(() => {
    const clock = setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => {
      clearInterval(clock);
    };
  });

  /**
   * The comment whose text is being changed: the text as it is now, and the
   * version it was begun from. `theirs`: saving was refused because somebody
   * else changed the comment since; their text is shown, and saving again
   * writes over it.
   */
  let editing = $state<{ id: string; text: string; readAt: number; theirs: boolean } | undefined>();
  let saving = $state(false);
  /** The comment whose earlier texts are open. */
  let historyOf = $state<string | undefined>();
  const edited = $derived(editing ? commentDoc(editing.text) : null);
  /** The comment being changed, as it is stored now. */
  const current = $derived(editing ? card.comment(editing.id) : undefined);

  let list = $state<HTMLElement>();

  /** Closes the field and gives the focus back to the comment's edit button. */
  async function close(id: string): Promise<void> {
    if (editing?.id !== id) return;
    editing = undefined;
    await tick();
    list
      ?.querySelector<HTMLElement>(`[data-comment="${CSS.escape(id)}"] [data-action="edit"]`)
      ?.focus();
  }

  /** `editing` as it is now (read after a wait, when it may have changed). */
  const editingNow = (): typeof editing => editing;

  async function save(): Promise<void> {
    if (!editing || !edited || saving) return;
    const { id, readAt } = editing;
    // The same text again is no change: sent, it would name the reader as the last to touch it.
    const stored = commentText(current?.body ?? null);
    if (stored !== undefined && JSON.stringify(commentDoc(stored)) === JSON.stringify(edited)) {
      await close(id);
      return;
    }
    saving = true;
    try {
      const outcome = await card.editComment(id, edited, readAt);
      if (outcome === 'saved') await close(id);
      else if (outcome === 'conflict') {
        // The field may have been closed, or another opened, while it was sent.
        const open = editingNow();
        if (open?.id === id) {
          // Shown now, theirs is what the next save knowingly writes over.
          open.theirs = true;
          open.readAt = card.comment(id)?.updatedAt ?? readAt;
        }
      }
      // Failed: the field stays, with what was written.
    } finally {
      saving = false;
    }
  }

  function onkeydown(event: KeyboardEvent): void {
    // A comment keeps no versions: the keys that keep one of a body only keep the browser
    // from saving the page, also while a syllable is being composed.
    if (isSaveKey(event)) {
      event.preventDefault();
      return;
    }
    if (isComposing(event)) return;
    if (event.key === 'Enter' && primaryModifier(event)) {
      event.preventDefault();
      void save();
    } else if (event.key === 'Escape' && editing) {
      // Leaves the field, not the card.
      event.preventDefault();
      event.stopPropagation();
      void close(editing.id);
    }
  }

  const context = $derived({
    locale: workspace.locale,
    timeZone: workspace.regional.timeZone,
    hourCycle: workspace.regional.hourCycle,
  });
</script>

<section
  aria-labelledby="{uid}-heading"
  class="flex flex-col gap-4 border-t pt-5"
  data-testid="card-comments"
>
  <svelte:element this={heading} id="{uid}-heading" class="font-semibold">
    {m['CardBadges.title-comments']()}
    <span class="font-normal text-muted-foreground">{card.comments.length}</span>
  </svelte:element>
  {#if card.comments.length === 0}
    <p class="text-muted-foreground">{m.card_comments_none()}</p>
  {/if}
  <ol
    bind:this={list}
    class="flex flex-col gap-4 empty:hidden"
    aria-live="polite"
    aria-relevant="additions"
  >
    {#each card.comments as comment (comment.id)}
      {@const author = workspace.userName(comment.createdBy) ?? m['centerPanel.unknown-user']()}
      {@const text = commentText(comment.body)}
      {@const agent = agentTokenNameOf(comment)}
      <!-- A long press that can react takes the press; one that cannot leaves the browser's own. -->
      {@const pressable = reactions.canReact && editing?.id !== comment.id}
      <li
        class="flex gap-3 rounded-md transition-colors data-revealed:bg-accent"
        data-comment={comment.id}
        {@attach pressable
          ? longPress(() => {
              reacting = comment.id;
            })
          : undefined}
      >
        <Avatar
          name={author}
          colorKey={comment.createdBy}
          locale={workspace.locale}
          decorative
          class="mt-0.5 size-7"
        />
        <div class="min-w-0 flex-1">
          <p class="flex flex-wrap items-baseline gap-x-2">
            <span class="font-medium">{author}</span>
            {#if agent}
              <span
                class="rounded-sm bg-muted px-1 text-xs font-medium"
                data-testid="comment-agent"
              >
                {m.agent_badge({ name: agent })}
              </span>
            {/if}
            <time
              class="text-xs text-muted-foreground"
              datetime={new Date(comment.createdAt).toISOString()}
              title={displayDateTime(comment.createdAt, context)}
            >
              {relativeTime(comment.createdAt, workspace.locale, Math.max(now, comment.createdAt))}
            </time>
            {#if comment.editedAt}
              {@const when = relativeTime(
                comment.editedAt,
                workspace.locale,
                Math.max(now, comment.editedAt),
              )}
              <span
                class="text-xs text-muted-foreground"
                title={displayDateTime(comment.editedAt, context)}
                data-testid="comment-edited"
              >
                <!-- A reader by the board's link is not told who. -->
                {comment.modifiedBy
                  ? m.card_comment_edited({ who: resolveUser(comment.modifiedBy), when })
                  : m.card_comment_edited_unnamed({ when })}
              </span>
            {/if}
          </p>
          {#if editing?.id === comment.id && board.canEditComment(comment.createdBy)}
            <!-- Kept only while the reader may still change it (a role can be taken away meanwhile). -->
            <form
              class="mt-1 flex flex-col gap-2"
              data-testid="comment-edit"
              onsubmit={(event) => {
                event.preventDefault();
                void save();
              }}
            >
              {#if editing.theirs}
                <div
                  class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm"
                  role="status"
                  data-testid="comment-theirs"
                >
                  <p class="font-medium">{m.card_comment_conflict()}</p>
                  <BodyView
                    doc={current?.body ?? null}
                    {resolveUser}
                    {references}
                    class="mt-1 text-sm"
                  />
                </div>
              {/if}
              <!-- The field opens on the reader's own press, to be typed in. -->
              <!-- svelte-ignore a11y_autofocus -->
              <textarea
                bind:value={editing.text}
                rows="1"
                autofocus
                aria-label={m.card_comment_edit_field()}
                aria-keyshortcuts={shortcutKeys('Enter')}
                class="field-sizing-content max-h-60 min-h-8 resize-none rounded-lg border border-input-border bg-transparent px-2.5 py-1 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30"
                {onkeydown}
                {@attach autosize(() => editing?.text ?? '')}></textarea>
              <div class="flex justify-end gap-2">
                <Button type="button" variant="ghost" onclick={() => void close(comment.id)}>
                  {m.shared_cancel()}
                </Button>
                <Button type="submit" disabled={!edited || saving}>
                  {m.card_comment_save()}
                </Button>
              </div>
            </form>
          {:else}
            <BodyView doc={comment.body} {resolveUser} {references} class="text-base">
              {#snippet invalid()}
                <p class="text-muted-foreground">{m.card_body_invalid()}</p>
              {/snippet}
            </BodyView>
          {/if}
          <CommentReactions
            class="mt-1.5"
            {reactions}
            blockId={comment.id}
            addLabel={m.reaction_add_to_comment({
              when: displayDateTime(comment.createdAt, context),
            })}
            bind:open={
              () => reacting === comment.id,
              (now: boolean) => {
                reacting = now ? comment.id : undefined;
              }
            }
          />
          {#if historyOf === comment.id && comment.editedAt && board.canReadCommentHistory}
            <CommentHistory
              id="{uid}-history-{comment.id}"
              read={readVersions}
              {references}
              commentId={comment.id}
              editedAt={comment.editedAt}
              body={comment.body}
              editedBy={comment.modifiedBy}
              {context}
              {resolveUser}
            />
          {/if}
        </div>
        {#if comment.editedAt && board.canReadCommentHistory}
          <IconButton
            label={m.card_comment_history({ when: displayDateTime(comment.createdAt, context) })}
            class={['shrink-0 text-muted-foreground', historyOf === comment.id && 'bg-accent']}
            aria-expanded={historyOf === comment.id}
            aria-controls={historyOf === comment.id ? `${uid}-history-${comment.id}` : undefined}
            data-action="history"
            onclick={() => {
              historyOf = historyOf === comment.id ? undefined : comment.id;
            }}
          >
            <HistoryIcon aria-hidden="true" />
          </IconButton>
        {/if}
        {#if text !== undefined && editing?.id !== comment.id && board.canEditComment(comment.createdBy)}
          <IconButton
            label={m.card_comment_edit_from({
              when: displayDateTime(comment.createdAt, context),
            })}
            class="shrink-0 text-muted-foreground"
            data-action="edit"
            onclick={() => {
              editing = { id: comment.id, text, readAt: comment.updatedAt, theirs: false };
            }}
          >
            <PencilIcon aria-hidden="true" />
          </IconButton>
        {/if}
        {#if board.canDeleteComment(comment.createdBy)}
          <IconButton
            label={m.card_comment_delete_from({
              when: displayDateTime(comment.createdAt, context),
            })}
            class="shrink-0 text-muted-foreground"
            onclick={() => {
              // An undo brings the comment back, not the edit.
              remove(comment.id);
            }}
          >
            <Trash2Icon aria-hidden="true" />
          </IconButton>
        {/if}
      </li>
    {/each}
  </ol>
</section>
