<!--
  The field for a new comment: the last row of the card, always in sight.
  It stays above the on-screen keyboard because the card ends where the
  keyboard begins (frame.ts), and clear of the home indicator when no
  keyboard covers that.

  A plain text box of several lines, not the editor: Enter starts a line,
  ⌘Enter on Apple devices, Ctrl+Enter elsewhere, or "Send" sends. Lines become paragraphs
  (comment-doc.ts). What was typed stays until it was stored; a comment that
  could not be stored is still in the field.

  A comment that is being written is not lost by leaving the card: Escape in
  the field only leaves the field (the next one closes the card), and a card
  that is closed with a draft has it again when it is opened later in the
  same visit (comment-drafts.ts).

  A wiki's page has the same field below its comments (`placement` `inline`),
  with the page's comments in place of the card.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import {
    isComposing,
    isSaveKey,
    primaryModifier,
    shortcutKeys,
    shortcutLabel,
  } from '$lib/keys.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardStore, CardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { autosize } from '$lib/components/autosize.ts';
  import { commentDoc } from './comment-doc.ts';
  import { draftOf, keepDraft } from './comment-drafts.ts';

  interface Props {
    /** The board, or the page's comments: who writes. */
    board: Pick<BoardStore, 'workspace'>;
    /** The card, or the page's comments: where the comment goes. */
    card: Pick<CardStore, 'addComment'>;
    /** Whose draft it is (`comment-drafts.ts`): the card's or the page's identifier. */
    draftKey: string;
    /** `pinned`: the last row of a card; `inline`: below the comments of a page. */
    placement?: 'pinned' | 'inline';
    /** A comment was stored: the card shows it. */
    onSent?: () => void;
  }

  let { board, card, draftKey, placement = 'pinned', onSent }: Props = $props();

  const workspace = $derived(board.workspace);
  const me = $derived(workspace.me);
  const myName = $derived(me ? (workspace.userName(me.id) ?? me.username) : '');

  const key = untrack(() => draftKey);
  let draft = $state(draftOf(key));
  let sending = $state(false);
  $effect(() => {
    keepDraft(key, draft);
  });
  const doc = $derived(commentDoc(draft));

  async function send(): Promise<void> {
    if (!doc || sending) return;
    sending = true;
    const sent = draft;
    const tell = onSent;
    try {
      const stored = await card.addComment(doc);
      if (stored) {
        // Typed on while it was being sent: that is the start of the next comment.
        if (draft.startsWith(sent)) draft = draft.slice(sent.length).trimStart();
        tell?.();
      }
    } finally {
      sending = false;
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
      void send();
    } else if (event.key === 'Escape' && draft.trim() !== '') {
      // With something written, Escape leaves the field and not the card.
      event.preventDefault();
      event.stopPropagation();
      (event.currentTarget as HTMLElement).blur();
    }
  }
</script>

<form
  class={[
    'flex shrink-0 items-end gap-2',
    placement === 'pinned'
      ? 'border-t bg-inherit px-3 pt-2 pb-[calc(0.5rem+max(0px,var(--safe-bottom)-var(--keyboard)))] @card-roomy/card:px-6'
      : 'pt-1',
  ]}
  data-testid="comment-composer"
  onsubmit={(event) => {
    event.preventDefault();
    void send();
  }}
>
  {#if me}
    <Avatar
      name={myName}
      colorKey={me.id}
      locale={workspace.locale}
      decorative
      class="mb-0.5 size-7 roomy:mb-2"
    />
  {/if}
  <textarea
    bind:value={draft}
    rows="1"
    aria-label={m['CardDetail.new-comment-placeholder']()}
    aria-keyshortcuts={shortcutKeys('Enter')}
    title={m.card_comment_send_hint({ shortcut: shortcutLabel('Enter') })}
    placeholder={m['CardDetail.new-comment-placeholder']()}
    data-testid="comment-field"
    class="field-sizing-content max-h-40 min-h-8 min-w-0 flex-1 resize-none rounded-lg border border-input-border bg-transparent px-2.5 py-1 text-sm outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30 roomy:min-h-touch roomy:py-2 roomy:text-base"
    {onkeydown}
    {@attach autosize(() => draft)}></textarea>
  <Button
    type="submit"
    class="roomy:min-h-touch"
    disabled={!doc || sending}
    data-testid="comment-send"
  >
    {m['CommentsList.send']()}
  </Button>
</form>
