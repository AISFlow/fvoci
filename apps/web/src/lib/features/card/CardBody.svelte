<!--
  The body of a card, edited together with whoever else has it open.

  The body is opened in its room (`CardStore.openBody`) and left when this
  goes away. The editor is bound to the room's document: what the reader
  types goes to the others as it is typed, and theirs comes in; where each of
  them is shows as a caret with their name. The document
  is the server's; until it has arrived (and the editor, which is not part
  of what the app loads first: editor-chunk.svelte.ts) the body is shown as
  the card was read, with the read-only view, so that a card can be read at
  once. Where there is no room (a reader by a public link) that view stays.

  The editor is read-only, with a line that says why, while the reader may
  only read the board, while the connection is away (it comes back by
  itself), while the document is being sent again, and when the room cannot
  be joined. When changes of the reader were dropped with their document,
  a notice says so until it is dismissed. Another document (the server made
  it anew) is another editor: the caret is put back where it was.

  A wiki's page has a body of the same kind, in a room of its own: it is
  shown by this too (`of="page"`), with the wiki in place of the board and
  the page in place of the card, and says so in its own words; an empty page
  says it is empty to whoever may only read it, also in the editor. So is the
  entry of a day of a diary (`of="entry"`), whose page may show, in place of
  the body as it was read, an editor of its own (`waiting`: the draft of an
  entry still being made) until the room's editor may take over (`holding`).

  ⌘S or Ctrl+S in the body keeps a version of it now (`keep`), and a line
  says what came of it for a while, aloud too. The browser's dialog to save
  the page never opens there, also in a body that only reads (which takes no
  focus: the keys count when what is selected is in it). Who only reads is
  told nothing; who edits a body that cannot be written for now is told that
  no version can be kept then.
-->
<script lang="ts">
  import type { Editor as TiptapEditor } from '@tiptap/core';
  import { tick, untrack, type Snippet } from 'svelte';
  import { on } from 'svelte/events';

  import { avatarColor } from '$lib/components/avatar/avatar.ts';
  import BodyView from '$lib/editor/BodyView.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isSaveKey } from '$lib/keys.ts';
  import { searchedNames } from '$lib/logic/person-names.ts';
  import type { BodyReferences } from '$lib/editor/body-editor.ts';
  import type { BoardViewHost } from '$lib/editor/board-view-read.ts';
  import type { BoardStore, CardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import {
    CARD_WORDS,
    ENTRY_WORDS,
    FORMULA_DROPPED,
    FILE_DROPPED,
    IMAGE_DROPPED,
    KEPT_SAID,
    LATE_NOTE,
    lostNote,
    PAGE_WORDS,
    roomNote,
    UNIT_WORDS,
    type BodyWords,
  } from './body-notes.ts';
  import type { KeepOutcome } from './body-versions.svelte.ts';
  import { keepCaret } from './caret-keeper.ts';
  import { editorChunk } from './editor-chunk.svelte.ts';

  interface Props {
    /**
     * The board, or the wiki: who may write, and who can be mentioned. One the reader is to
     * join by writing (`writeJoined`) is joined as the caret is put in its body, which lets them
     * write.
     */
    container: Pick<BoardStore, 'workspace' | 'canEdit' | 'orderedMembers'> & {
      writeJoined?: <T>(write: () => Promise<T>) => Promise<T>;
    };
    /** The card, or the wiki's page: the body and its room. */
    card: Pick<
      CardStore,
      | 'room'
      | 'body'
      | 'lost'
      | 'openBody'
      | 'closeBody'
      | 'retryBody'
      | 'sayLost'
      | 'dismissLost'
      | 'uploadFile'
    >;
    /** Whose body it is, for the words it says: a card's, a wiki's page's, a diary's entry's, or a course's unit's. */
    of?: 'card' | 'page' | 'entry' | 'unit';
    /** Shown in place of the body as it was read, until the room's editor is there. */
    waiting?: Snippet<[onSave: () => void]> | undefined;
    /** The room's editor waits while this is true: `waiting` stays. */
    holding?: boolean;
    /** The room's editor was made (for each document). */
    onEditor?: ((editor: TiptapEditor) => void) | undefined;
    /** What its references are drawn from and put with (`bodyReferences`); without it, none is shown. */
    references?: BodyReferences | undefined;
    /** What draws and puts in its board views: a card's, a page's or a unit's body; never an entry's. */
    boardViews?: BoardViewHost | undefined;
    /** Keeps a version of the body now (`keepVersion`); without it, the keys keep none. */
    keep?: (() => Promise<KeepOutcome>) | undefined;
    testId?: string;
  }

  let {
    container,
    card,
    of = 'card',
    testId = 'card-body',
    references,
    boardViews,
    keep,
    waiting,
    holding = false,
    onEditor,
  }: Props = $props();

  /** How long what came of the keys is shown: long enough to read, gone before it is old news. */
  const KEPT_SHOWN = 6000;

  const words = $derived(
    { card: CARD_WORDS, page: PAGE_WORDS, entry: ENTRY_WORDS, unit: UNIT_WORDS }[
      of
    ] satisfies BodyWords,
  );

  const workspace = $derived(container.workspace);
  const room = $derived(card.room);
  const doc = $derived(room?.doc);
  /** The carets of the room's document, each in the colour its person's picture has. */
  const carets = $derived(
    room?.carets && {
      awareness: room.carets,
      colorOf: (userId: string) => `var(--option-${avatarColor(userId)})`,
    },
  );
  const Editor = $derived(room ? editorChunk.component : undefined);
  /** The reader may write: the board lets them, and the room does now. */
  const writable = $derived(container.canEdit && (room?.writable ?? false));
  const note = $derived(room ? roomNote(room, container.canEdit, words) : undefined);
  /** The room is being joined, or its document sent again: what is shown is about to change. */
  const settling = $derived(room?.status === 'joining' || room?.status === 'receiving');

  // Somebody not known (gone, or a mention that names nobody, as an anonymous reader gets them).
  const resolveUser = (userId: string): string =>
    workspace.userName(userId) ?? m['centerPanel.unknown-user']();
  /** Who can be mentioned: the members of the board, found by any of their names. */
  const people = $derived(
    container.orderedMembers.map((member) => {
      const user = workspace.user(member.userId);
      return {
        id: member.userId,
        name: workspace.userName(member.userId) ?? m['centerPanel.unknown-user'](),
        also: user ? [user.username, ...(user.name ? searchedNames(user.name) : [])] : [],
      };
    }),
  );

  /** The workspace the room is asked for in; the room opens once it is known. */
  const workspaceId = $derived(workspace.info?.id);

  // The room is the card's while this is shown; going away leaves it. Only
  // another card (or the workspace becoming known) opens another room:
  // nothing the opening reads does.
  $effect(() => {
    const store = card;
    if (workspaceId === undefined) return;
    untrack(() => {
      store.openBody();
    });
    return () => {
      store.closeBody();
    };
  });

  $effect(() => {
    if (room) editorChunk.load();
  });

  const caret = keepCaret();
  /** How many times the editor was made anew for the same document. */
  let made = $state(0);

  /** Joining and receiving are usually over at once: their line is put in only when they last. */
  let lateShown = $state(false);
  const late = $derived(note?.late ?? false);
  /** The line said under the body: none while the editor's code failed (that says so itself). */
  const shownNote = $derived(
    !(room && editorChunk.failed) && note && (!note.late || lateShown) ? note : undefined,
  );
  /** What came of the keys that keep a version, said aloud and shown for a while. */
  let keptSaid = $state('');
  let keptTimer: ReturnType<typeof setTimeout> | undefined;
  /** A version is being kept: the keys held or pressed again meanwhile ask nothing more. */
  let keeping = false;
  /** The body went away (the card was closed): what comes of a keeping after it is said nowhere. */
  let gone = false;

  async function keepNow(): Promise<void> {
    if (!keep || !container.canEdit || keeping) return;
    keeping = true;
    try {
      say(KEPT_SAID[await keep()]());
    } finally {
      keeping = false;
    }
  }

  /** Words said again are said again: the line is emptied first. */
  function say(text: string): void {
    clearTimeout(keptTimer);
    keptSaid = '';
    void tick().then(() => {
      if (gone) return;
      keptSaid = text;
      keptTimer = setTimeout(() => {
        keptSaid = '';
      }, KEPT_SHOWN);
    });
  }
  $effect(() => () => {
    gone = true;
    clearTimeout(keptTimer);
  });

  /**
   * The keys pressed in the body but not in its editor (which takes them itself), heard
   * from the page while the body is there: on something in it, or on what holds it (the
   * card's dialog, the page, where the focus goes from a body that takes none) with what
   * is selected in it. Not on anything beside it: a dialog over it, another field.
   */
  const keysOutside = (body: HTMLElement): (() => void) =>
    on(document, 'keydown', (event) => {
      const target = event.target;
      if (event.defaultPrevented || !isSaveKey(event) || !(target instanceof Node)) return;
      const selected = document.getSelection()?.focusNode ?? null;
      const inBody =
        body.contains(target) ||
        (target.contains(body) && selected !== null && body.contains(selected));
      if (!inBody) return;
      event.preventDefault();
      if (!event.repeat) void keepNow();
    });

  /**
   * Pressed in the editor to put the caret there, where the reader may write once joined and the
   * room still reads: joins. Not a press that selected words (to copy them), nor one on a control
   * in the body (a link, a file's buttons), nor the focus passing through by the keyboard. The
   * editor that reads takes no focus: the keyboard joins by a write elsewhere (a title, a comment).
   */
  const joinOnPress = (body: HTMLElement): (() => void) =>
    on(body, 'click', (event) => {
      const target = event.target;
      // The editor's root, by its own mark: its library's class would carry the library's name
      // into this piece, which is fetched before the editor (e2e/card.spec.ts).
      if (!(target instanceof Element) || !target.closest('[data-body-editor]')) return;
      if (target.closest('a[href], button, input, select, textarea, [role="button"]')) return;
      if (document.getSelection()?.isCollapsed === false) return;
      if (container.canEdit && room?.status === 'joined' && room.mode === 'read') {
        void container.writeJoined?.(() => Promise.resolve());
      }
    });

  $effect(() => {
    if (!late) return;
    const timer = setTimeout(() => {
      lateShown = true;
    }, LATE_NOTE);
    return () => {
      clearTimeout(timer);
      lateShown = false;
    };
  });
</script>

<section
  {@attach keysOutside}
  {@attach joinOnPress}
  aria-label={words.label()}
  class="flex flex-col gap-3 border-t pt-5"
  data-testid={testId}
  data-editor={Editor && doc && !holding
    ? writable
      ? 'ready'
      : 'reading'
    : room
      ? 'loading'
      : 'none'}
  data-room={room?.status ?? 'none'}
  data-settling={settling ? '' : undefined}
  data-pending={room?.pending ? '' : undefined}
>
  {#if card.lost.length > 0}
    <div
      class="flex flex-wrap items-center gap-2 rounded-lg border border-destructive/40 bg-destructive/10 p-3"
      role="alert"
      data-testid="body-lost"
    >
      <div class="min-w-0 flex-1">
        {#each card.lost as why (why)}
          <p>{lostNote(why, words)}</p>
        {/each}
      </div>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        onclick={() => {
          card.dismissLost();
        }}
      >
        {m.ui_close()}
      </Button>
    </div>
  {/if}
  {#if Editor && doc && !holding}
    <!-- Made anew for another document, and when one it could not draw fits again. -->
    {#key made}
      {#key doc}
        <Editor
          doc={card.body}
          collab={doc}
          {carets}
          onComposing={room?.composing}
          readsFirst={room?.readsFirst}
          editable={writable}
          canWrite={container.canEdit && room?.mode === 'write'}
          onFitsAgain={() => {
            made += 1;
          }}
          onUploadDropped={(kind) => {
            card.sayLost(kind === 'file' ? FILE_DROPPED : IMAGE_DROPPED);
          }}
          onDraftDropped={() => {
            card.sayLost(FORMULA_DROPPED);
          }}
          onSave={() => void keepNow()}
          users={people}
          {resolveUser}
          {references}
          boardViews={of === 'entry' ? undefined : boardViews}
          label={words.label()}
          placeholder={words.placeholder()}
          readPlaceholder={words.readEmpty?.() ?? ''}
          class="min-h-24 text-base outline-none"
          uploadFile={(file: File) => card.uploadFile(file)}
          onEditor={(editor: TiptapEditor) => {
            caret.take(editor, writable);
            onEditor?.(editor);
          }}
        />
      {/key}
    {/key}
  {:else if waiting}
    {@render waiting(() => void keepNow())}
  {:else}
    <div {@attach caret.watch}>
      <BodyView
        doc={card.body}
        {resolveUser}
        {references}
        boardViews={of === 'entry' ? undefined : boardViews}
        class="text-base"
      >
        {#snippet empty()}
          <p class="text-base text-muted-foreground">
            {container.canEdit ? words.placeholder() : words.empty()}
          </p>
        {/snippet}
        {#snippet invalid()}
          <p class="text-muted-foreground" role="alert">{words.invalid()}</p>
        {/snippet}
      </BodyView>
    </div>
  {/if}
  {#if room && editorChunk.failed}
    <p class="flex flex-wrap items-center gap-2 text-muted-foreground" role="alert">
      {m.card_body_editor_failed()}
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        onclick={() => {
          editorChunk.load();
        }}
      >
        {m.card_body_editor_retry()}
      </Button>
    </p>
  {/if}
  <!-- The line is in a live region that is always there, so that it is said aloud; its button is not. -->
  <div class="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
    <p role="status">
      {#if shownNote}
        <span data-testid="body-room-note" data-reason={room?.reason}>{shownNote.text}</span>
      {/if}
    </p>
    <p role="status" data-testid="body-kept">{keptSaid}</p>
    {#if shownNote?.retry}
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        onclick={() => {
          card.retryBody();
        }}
      >
        {m.card_body_editor_retry()}
      </Button>
    {/if}
  </div>
</section>
