<!--
  The editor of a document body: the description of a card, a comment.

  Everything comes in as values and callbacks; nothing here knows the server
  or the app's state. The caller fetches this component when a body is first
  edited (`await import('$lib/editor/BodyEditor.svelte')`): the editing
  library is large and no part of the first screen.

  Bound to a shared document (`collab`, a card's body in its room), the
  editor shows what the document holds and writes into it: `doc`,
  `revision`, `onLoad` and `onChange` are not read. With `carets` it shows
  where the others who edit it are (carets.ts). What the editor cannot
  draw is kept, the editor says so and stops editing until it fits again.

  Without one (the stories, and the editor's own tests), the document is
  read when the component is made and again whenever `revision` goes up,
  and at no other time: a change of `doc` alone never overwrites what is
  being written. Every edit is reported through `onChange` in the stored
  form, `null` for an empty body. A body the validator refuses is not
  loaded: editing it would mean saving the editor's guess at what it meant
  over the original. The editor then stays empty and read-only and says so.

  Around the text: `/` offers the kinds of block (block-menu.ts); chosen
  text gets a bubble of marks, and on a phone a bar over the keyboard does
  what the bubble and the menu do (EditorTools.svelte). An image opens
  larger from its bubble, or on a press while the editor is read-only.
-->
<script lang="ts">
  import { Editor } from '@tiptap/core';
  import type { Transaction } from '@tiptap/pm/state';
  import { onMount, untrack } from 'svelte';
  import { on } from 'svelte/events';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';

  import { slashEntries } from './block-menu.ts';
  import { boardViewEntry, type BoardViewOptions } from './board-view.ts';
  import EditorTools from './EditorTools.svelte';
  import { BODY_FIELD, type BodyEditorProps, type BodyReferences } from './body-editor.ts';
  import { CheckedCollaboration, hasInvalidContent } from './content-check.ts';
  import { drawReferences } from './reference-chip.ts';
  import { fromShared, setMayWrite, type DroppedWhy } from './shared-place.ts';
  import { caretsOf } from './carets.ts';
  import './carets.css';
  import './editor.css';
  import './editing-place.css';
  import { loadEmoji, searchEmoji } from './emoji.ts';
  import { bodyExtensions, type MentionOptions } from './extensions.ts';
  import { heldWritten, placeOf } from './editing-place.ts';
  import { cancelMathEditingOf, mathDraftChanged } from './math-editing.ts';
  import { MovesKept } from './moves-kept.ts';
  import type { ReferenceOptions } from './reference.ts';
  import { emptyDoc, isEmptyDoc, toStoredDoc, validateDoc, type DocNode } from './schema.ts';
  import { choiceList, suggestionList } from './suggestion-list.ts';
  import { SlashMenu } from './slash-menu.ts';
  import { BodyUpload, fromUpload } from './uploads.ts';
  import { SaveKey } from './save-key.ts';

  let {
    doc,
    revision = 0,
    editable = true,
    users = [],
    resolveUser,
    references,
    boardViews,
    onChange,
    onFocus,
    onBlur,
    uploadFile,
    placeholder = '',
    readPlaceholder = '',
    class: className,
    label,
    toolbar,
    onLoad,
    collab,
    carets,
    onComposing,
    readsFirst,
    onEditor,
    canWrite = false,
    onUploadDropped,
    onFitsAgain,
    onDraftDropped,
    onSave,
  }: BodyEditorProps = $props();

  let host: HTMLDivElement;
  let editor = $state.raw<Editor>();
  let changes = $state(0);
  /** Whether the document that was last read could not be loaded. */
  let unreadable = $state(false);
  /**
   * The last thing that came of something the reader did and is to be said:
   * an image or a file block not put in (`upload`), a formula's source not
   * kept (which an upload's does not replace). Gone once the reader writes
   * again.
   */
  let said = $state<{ text: string; upload: boolean }>();

  const status = $derived(unreadable ? m.editor_body_unreadable() : (said?.text ?? ''));

  /**
   * What is said of a file that went up and was not put in. One going up, or
   * one that did not go up, is told where every upload is (the shell's stack).
   */
  const UPLOAD_SAID = {
    image: { gone: m.editor_image_gone, refused: m.editor_image_refused },
    file: { gone: m.editor_file_gone, refused: m.editor_file_refused },
  };

  const nameOf = (userId: string): string =>
    resolveUser?.(userId) ?? users.find((user) => user.id === userId)?.name ?? userId;

  /** The document to load for a stored body; undefined when it must not be edited. */
  function loadable(stored: DocNode | null): DocNode | undefined {
    if (stored === null) return emptyDoc();
    return validateDoc(stored).length === 0 ? stored : undefined;
  }

  /** Whether a shared document the editor could not draw fits it again. */
  function fitsAgain(current: Editor): boolean {
    return !hasInvalidContent(current.state);
  }

  /** What an editor holds, as it would be stored; `null` for an empty body. */
  function storedOf(current: Editor): DocNode | null {
    const stored = toStoredDoc(current.getJSON() as DocNode);
    return isEmptyDoc(stored) ? null : stored;
  }

  const rootAttributes = (): Record<string, string> => ({
    class: ['doc-body', className].filter(Boolean).join(' '),
    'aria-label': label ?? m.editor_body_label(),
    'aria-multiline': 'true',
    // A field for the app's keys even while read-only for now (undo-keys.ts).
    'data-body-editor': '',
  });

  /**
   * The editor's own events, run as from outside any effect. A focused
   * editor that is taken out of the page (its card closes) blurs while
   * Svelte removes it, inside the effect that removes it; a state change
   * there is otherwise refused (`state_unsafe_mutation`).
   */
  const fromEditor =
    <A extends unknown[]>(handle: (...args: A) => void) =>
    (...args: A): void => {
      untrack(() => {
        handle(...args);
      });
    };

  const mention: MentionOptions = {
    search: (query) => {
      const wanted = query.toLowerCase();
      return users
        .filter((user) =>
          [user.name, ...(user.also ?? [])].some((name) => name.toLowerCase().includes(wanted)),
        )
        .slice(0, 8)
        .map((user) => ({ value: user.id, label: user.name }));
    },
    labelOf: nameOf,
    render: suggestionList({ kind: 'mention', label: m.editor_suggest_people }),
  };

  /** Each asks `references` as it is when asked: the editor outlives a change of it. */
  const reference: ReferenceOptions = {
    drawing: () => references,
    search: (query) => references?.search(query) ?? [],
    numbered: (query) => references?.numbered?.(query) ?? [],
    named: (address) => references?.named(address),
    seen: (id) => references?.seen(id) ?? Promise.resolve(false),
    render: suggestionList({ kind: 'reference', label: m.editor_suggest_references }),
    renderNumbered: suggestionList({ kind: 'card-number', label: m.editor_suggest_board_cards }),
  };

  /**
   * Files pasted, dropped or chosen go up through `uploadFile` as they are asked; what is
   * said of one that went up and ends otherwise than in the body.
   */
  const uploading = BodyUpload.configure({
    upload: () => uploadFile,
    uploadingLabel: (kind) =>
      kind === 'file' ? m.editor_file_uploading() : m.editor_image_uploading(),
    onProgress: (ended, destroyed) => {
      if (destroyed) {
        if (ended.outcome === 'gone') onUploadDropped?.(ended.kind);
        return;
      }
      if (ended.outcome !== 'gone' && ended.outcome !== 'refused') return;
      // A formula's loss, said first, is not replaced.
      if (!(said && !said.upload)) {
        said = { text: UPLOAD_SAID[ended.kind][ended.outcome](), upload: true };
      }
    },
  });

  /**
   * The board views of a body that may show them: drawn by the app, put in by
   * the slash menu and by pasting a view's address (board-view.ts). Read when
   * the editor is made: a body that may show them always may.
   */
  const viewHost = untrack(() => boardViews);
  const boardView: BoardViewOptions = viewHost
    ? {
        ...viewHost,
        allowed: true,
        choices: choiceList({ kind: 'board-view', label: m.editor_board_view_paste_label }),
      }
    : { allowed: false };
  const viewEntries = viewHost ? boardViewEntry(viewHost.choose) : undefined;

  /** The slash menu: the kinds of block, tables, board views, and a file where files can be added (block-menu.ts). */
  const slashMenu = SlashMenu.configure({
    entries: (current) => slashEntries(current, uploadFile !== undefined, viewEntries),
    render: suggestionList({ kind: 'slash', label: m.editor_slash_label }),
  });

  /** What binds the editor to a shared document, keeps the reader's moves through what the others do, and shows their carets. */
  const sharing = (shared: NonNullable<BodyEditorProps['collab']>) => [
    CheckedCollaboration.configure({ document: shared, field: BODY_FIELD }),
    MovesKept.configure({ readsFirst }),
    ...(carets ? [caretsOf({ ...carets, nameOf })] : []),
  ];

  onMount(() => {
    // Bound to a shared document, the editor takes what is in it; `doc` is not read.
    const shared = collab;
    const first = shared ? emptyDoc() : loadable(doc);
    unreadable = first === undefined;
    const created = new Editor({
      element: host,
      // The styles the library would put into the page are in editor.css.
      injectCSS: false,
      editable: editable && first !== undefined,
      ...(shared
        ? {
            // What the editor cannot draw (written by a newer version of the app) is
            // kept, not deleted for everyone: CheckedCollaboration validates the whole
            // body once on load and changed ranges thereafter, stopping editing until
            // the shared document fits again (the server mends what it may).
            onContentError: fromEditor(() => {
              unreadable = true;
            }),
          }
        : { content: first ?? emptyDoc() }),
      extensions: [
        ...(shared ? sharing(shared) : []),
        ...bodyExtensions({
          history: !shared,
          placeholder,
          readPlaceholder,
          mention,
          reference,
          boardView,
          emoji: {
            search: async (query) => {
              const found = searchEmoji(await loadEmoji(getLocale()), query);
              return found.map(({ emoji, name }) => ({ value: emoji, label: `${emoji} ${name}` }));
            },
            render: suggestionList({ kind: 'emoji', label: m.editor_suggest_emoji }),
          },
        }),
        uploading,
        slashMenu,
      ],
      editorProps: { attributes: rootAttributes() },
      onTransaction: fromEditor(({ transaction }: { transaction: Transaction }) => {
        changes += 1;
        // The reader writes again: what was said of before is over.
        const reader = !fromShared(transaction) && !fromUpload(transaction);
        if (transaction.docChanged && reader) said = undefined;
      }),
      onUpdate: fromEditor(({ editor: current }: { editor: Editor }) =>
        onChange?.(storedOf(current)),
      ),
      onFocus: fromEditor(() => onFocus?.()),
      onBlur: fromEditor(() => onBlur?.()),
    });
    editor = created;
    setMayWrite(created.view, {
      may: () => shared !== undefined && canWrite,
      dropped: fromEditor((why: DroppedWhy) => {
        const text = why === 'gone' ? m.editor_formula_gone() : m.editor_formula_refused();
        said = { text, upload: false };
      }),
    });
    // What it could not draw is gone again: the editor, which took nothing from the shared
    // document meanwhile, is made anew from it (never made editable on what it last showed).
    const fits = (): void => {
      if (unreadable && fitsAgain(created)) onFitsAgain?.();
    };
    shared?.on('afterTransaction', fits);
    if (!shared) onLoad?.(first === undefined ? undefined : storedOf(created));
    onEditor?.(created);
    return () => {
      shared?.off('afterTransaction', fits);
      // Made anew for another document: what is still in a formula's field cannot go into it.
      const place = placeOf(created.state);
      const unsent = mathDraftChanged(created.view) || (place !== null && heldWritten(place));
      if (shared && unsent) fromEditor(() => onDraftDropped?.())();
      leaveFields(created);
      created.destroy();
    };
  });

  // A change from outside: read `doc` again. Not on the first run, which the
  // editor's own creation covers, and never because `doc` itself changed.
  // A shared document is its own: nothing is read into it from outside.
  let seen: number | undefined;
  $effect(() => {
    const next = revision;
    const current = editor;
    if (!current || untrack(() => collab)) return;
    if (seen === undefined || seen === next) {
      seen = next;
      return;
    }
    seen = next;
    untrack(() => {
      const fresh = loadable(doc);
      unreadable = fresh === undefined;
      cancelMathEditingOf(current.view);
      // A received revision is not the reader's edit to undo with their next word.
      current
        .chain()
        .setContent(fresh ?? emptyDoc(), { emitUpdate: false })
        .setMeta('addToHistory', false)
        .run();
      current.setEditable(editable && fresh !== undefined, false);
      onLoad?.(fresh === undefined ? undefined : storedOf(current));
    });
  });

  $effect(() => {
    editor?.setEditable(editable && !unreadable, false);
  });

  $effect(() => {
    const attributes = rootAttributes();
    editor?.setOptions({ editorProps: { attributes } });
  });

  /** A syllable is being composed in the body: said to whoever holds what comes from outside. */
  let composing = false;
  const saveKey = new SaveKey(() => onSave);
  /** The keys of the editor's own text, as they come up from it (the field is the editor inside). */
  const saveKeys = (element: HTMLElement): (() => void) =>
    on(element, 'keydown', (event) => {
      saveKey.down(event, composing);
    });

  /** The editor goes away: a formula's field with it, and the syllable it was in the middle of. */
  function leaveFields(leaving: Editor): void {
    cancelMathEditingOf(leaving.view);
    if (composing) fromEditor(() => onComposing?.(false))();
    composing = false;
    saveKey.drop();
  }

  /** A stroke of a syllable in the body's own text (not in a field drawn into it: a formula's). */
  function composes(event: CompositionEvent): void {
    if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement) {
      return;
    }
    composing = true;
    onComposing?.(true);
  }

  /**
   * The composition is over. Said once the editor has read what it left in
   * the page, which it does right after this (in a microtask of its own,
   * queued before this one), and before whatever is typed next.
   */
  function composed(): void {
    if (!composing) return;
    queueMicrotask(() => {
      composing = false;
      onComposing?.(false);
      saveKey.composed();
    });
  }

  // Names that become known later: a mention shows the name it had when it
  // was drawn, so the ones in the page are brought up to date.
  function renameMentions(
    current: Editor | undefined,
    name: (userId: string) => string,
    ..._known: unknown[]
  ): void {
    if (!current) return;
    for (const mention of current.view.dom.querySelectorAll<HTMLElement>(
      'span[data-mention][data-user-id]',
    )) {
      const shown = `@${name(mention.dataset.userId ?? '')}`;
      if (mention.textContent !== shown) mention.textContent = shown;
    }
  }
  $effect(() => {
    // Handing both over is what makes this run again when either changes.
    renameMentions(editor, nameOf, users, resolveUser);
  });

  // What a reference names is drawn into it, and again whenever what is known
  // of it changes; `changes` brings the references a change of the document put.
  function drawReferencesOf(
    current: Editor | undefined,
    drawing: BodyReferences | undefined,
    ..._changed: unknown[]
  ): void {
    if (current) drawReferences(current.view.dom, drawing);
  }
  $effect(() => {
    drawReferencesOf(editor, references, changes);
  });
</script>

<div class="doc-editor" data-unreadable={unreadable || undefined}>
  {#if toolbar && editor}
    {@render toolbar({ editor, changes })}
  {/if}
  <div
    bind:this={host}
    class="doc-editor-text"
    oncompositionstart={composes}
    oncompositionupdate={composes}
    oncompositionend={composed}
    {@attach saveKeys}
  ></div>
  <!-- Said aloud when it changes: an upload under way, an upload that failed, a body that cannot be edited. -->
  <p class="doc-editor-status" role="status" data-shown={status === '' ? undefined : ''}>
    {status}
  </p>
  {#if editor}
    <EditorTools
      {editor}
      {changes}
      editable={editable && !unreadable}
      uploads={uploadFile !== undefined}
    />
  {/if}
</div>
