/**
 * A reference in a body: the identifier of a card or a page of the
 * workspace, and nothing else. What it names, and whether the reader may see
 * it, is asked where the node is drawn (reference-chip.ts), so a title is
 * never part of a document: a renamed card shows under its new name, a
 * reader who may not see it is shown no name at all, and what is copied out
 * of the editor carries the identifier alone.
 *
 * A reference is put in four ways: by choosing from what `[[` offers, or
 * from the cards of the same board that `#` and a number or words of a title
 * offer (the number is the card's, never the document's), by pasting the
 * address of a card or a page of the workspace that the reader may see, and
 * by pasting what was copied out of a body. What is pasted from
 * a body is taken as it is: a writer may name what they cannot see, as the
 * server takes any identifier, and readers are shown what each may see.
 */
import { mergeAttributes, Node } from '@tiptap/core';
import { Plugin, PluginKey, type EditorState, type Transaction } from '@tiptap/pm/state';
import type { Node as ProseMirrorNode } from '@tiptap/pm/model';
import type { EditorView } from '@tiptap/pm/view';
import { Suggestion, type SuggestionMatch, type Trigger } from '@tiptap/suggestion';

import { REFERENCE_CLASS, REFERENCE_ID_ATTRIBUTE } from './drawing.ts';
import type { SuggestionEntry, SuggestionRenderer } from './extensions.ts';
import { drawReference, openClickedReference, type ReferenceDrawing } from './reference-chip.ts';
import { COPIED_CARD_LINK } from '$lib/logic/card-numbers.ts';

import { isBlockId } from './schema.ts';
import { keepTouchFromText } from './touch-press.ts';

export interface ReferenceOptions {
  /** What the references are drawn from, as it is now (reference-chip.ts). */
  drawing: () => ReferenceDrawing | undefined;
  /** What may be named whose title matches what was typed after `[[`. */
  search: (query: string) => SuggestionEntry[] | Promise<SuggestionEntry[]>;
  /** The cards of the same board that what was typed after `#` names; none where there is no board. */
  numbered: (query: string) => SuggestionEntry[];
  /** The identifier an address names, if it is the address of a card or a page of this workspace. */
  named: (address: string) => string | undefined;
  /** Whether the reader may see what the identifier names: only then is a pasted address made a reference. */
  seen: (id: string) => Promise<boolean>;
  render?: SuggestionRenderer;
  /** The list of what `#` offers. */
  renderNumbered?: SuggestionRenderer;
}

/** The longest title that is looked for after `[[`. */
const LONGEST_QUERY = 80;

/**
 * What is being typed after `[[` at the cursor, if anything is: up to the
 * cursor, spaces included (a title has them), within the line. A `]` ends
 * it, and so does a node that is not text. Not in code.
 */
export function referenceMatch({ $position }: Trigger): SuggestionMatch {
  if ($position.parent.type.spec.code) return null;
  if ($position.marks().some((mark) => mark.type.spec.code)) return null;
  const before = $position.parent.textBetween(0, $position.parentOffset, undefined, '￼');
  const index = before.lastIndexOf('[[');
  if (index === -1) return null;
  const query = before.slice(index + 2);
  if (query.length > LONGEST_QUERY || /[\]￼\n]/.test(query)) return null;
  return {
    range: { from: $position.start() + index, to: $position.pos },
    query,
    text: `[[${query}`,
  };
}

/**
 * What is being typed after `#` at the cursor, if anything is: up to the
 * cursor within the line, a number or the words of a title (`#12`, `#두
 * 단어`). The `#` begins a word: at the start of the line, or after a space
 * or a `(`; not `C#`, an address's `#part`, `##`, or `[[#`. What follows it
 * begins with no space (`# ` makes a heading), and a `]`, a `#` or a node
 * that is not text ends it. Not in code, and not within what `[[` is
 * taking (`[[Bug #1`): one list at a time.
 */
export function cardNumberMatch(trigger: Trigger): SuggestionMatch {
  const { $position } = trigger;
  if (referenceMatch(trigger)) return null;
  if ($position.parent.type.spec.code) return null;
  if ($position.marks().some((mark) => mark.type.spec.code)) return null;
  const before = $position.parent.textBetween(0, $position.parentOffset, undefined, '￼');
  const index = before.lastIndexOf('#');
  if (index === -1 || !/(^|[\s(])$/u.test(before.slice(0, index))) return null;
  const query = before.slice(index + 1);
  if (query === '' || query.length > LONGEST_QUERY || /^\s|[\]#￼\n]/u.test(query)) return null;
  return {
    range: { from: $position.start() + index, to: $position.pos },
    query,
    text: `#${query}`,
  };
}

export const Reference = Node.create<ReferenceOptions>({
  name: 'reference',
  group: 'inline',
  inline: true,
  atom: true,

  addOptions() {
    return {
      drawing: () => undefined,
      search: () => [],
      numbered: () => [],
      named: () => undefined,
      seen: () => Promise.resolve(false),
    };
  },

  addAttributes() {
    return {
      id: {
        default: null,
        parseHTML: (element: HTMLElement) => element.getAttribute(REFERENCE_ID_ATTRIBUTE),
        renderHTML: (attrs: { id?: string | null }) => ({ [REFERENCE_ID_ATTRIBUTE]: attrs.id }),
      },
    };
  },

  parseHTML() {
    // A reference that names nothing (pasted from elsewhere, or from a view a
    // reader by a link got) is not one: the server would refuse the body.
    return [
      {
        tag: `span[data-reference][${REFERENCE_ID_ATTRIBUTE}]`,
        getAttrs: (element: HTMLElement) =>
          isBlockId(element.getAttribute(REFERENCE_ID_ATTRIBUTE)) ? null : false,
      },
    ];
  },

  // What is copied out and what `getHTML` gives: the identifier, and no title.
  renderHTML({ HTMLAttributes }) {
    return [
      'span',
      mergeAttributes(HTMLAttributes, { 'data-reference': '', class: REFERENCE_CLASS }),
    ];
  },

  // The title is not the document's: copied as text, a reference is nothing.
  renderText() {
    return '';
  },

  // What the reader sees: the same element, with the title drawn into it.
  addNodeView() {
    return ({ HTMLAttributes }) => {
      const dom = document.createElement('span');
      const attributes = mergeAttributes(HTMLAttributes, {
        'data-reference': '',
        class: REFERENCE_CLASS,
      });
      for (const [name, value] of Object.entries(attributes)) dom.setAttribute(name, String(value));
      drawReference(dom, this.options.drawing());
      dom.addEventListener('click', (event) => {
        openClickedReference(event, this.options.drawing());
      });
      // A finger opens what it names without the body taking the focus (touch-press.ts).
      keepTouchFromText(dom);
      // No part of it is the document's: what is drawn inside is no change of it.
      return { dom };
    };
  },

  addProseMirrorPlugins() {
    const { editor, name, options } = this;
    /** A list of what may be named, after `char`: what is taken is put as a reference and a space. */
    const offering = (
      char: string,
      findSuggestionMatch: (trigger: Trigger) => SuggestionMatch,
      items: (query: string) => SuggestionEntry[] | Promise<SuggestionEntry[]>,
      render: SuggestionRenderer | undefined,
    ) =>
      Suggestion<SuggestionEntry, SuggestionEntry>({
        editor,
        pluginKey: new PluginKey(`referenceSuggestion${char}`),
        char,
        findSuggestionMatch,
        items: ({ query }) => items(query),
        command: ({ editor: current, range, props }) => {
          const reference = { type: name, attrs: { id: props.value } };
          current
            .chain()
            .focus()
            .insertContentAt(range, [reference, { type: 'text', text: ' ' }])
            .run();
        },
        ...(render ? { render } : {}),
      });
    return [
      offering('[[', referenceMatch, (query) => options.search(query), options.render),
      offering('#', cardNumberMatch, (query) => options.numbered(query), options.renderNumbered),
      pastedAddresses(name, options),
    ];
  },
});

/** An address that was pasted, where it is now, while whether the reader may see what it names is asked. */
interface PastedAddress {
  /** Which paste it is: two pastes of one address are two. */
  key: number;
  from: number;
  to: number;
  text: string;
  id: string;
}

type PasteMeta = { add: PastedAddress } | { done: number };

const pasteKey = new PluginKey<PastedAddress[]>('referencePaste');

/** Whether the selection is in code, a block of it or a mark of it: there an address is text. */
function inCode(state: EditorState): boolean {
  const { $from } = state.selection;
  return Boolean($from.parent.type.spec.code) || $from.marks().some((mark) => mark.type.spec.code);
}

/**
 * A card's link copied in this app, pasted where it is not made a reference
 * (over a selection, in code, an address of no card here): its address, as
 * text, as it is pasted from anywhere else. The words that the link carries
 * for other apps, the card's number among them, never go into a body.
 */
function copiedCardLink(view: EditorView, event: ClipboardEvent, text: string): boolean {
  const html = event.clipboardData?.getData('text/html') ?? '';
  if (text === '' || !html.includes(COPIED_CARD_LINK)) return false;
  // Pasted with no clipboard of its own, it does not come here again.
  view.pasteText(text);
  return true;
}

/** Whether any of `from`–`to` is code, a block of it or a mark of it. */
function codeBetween(doc: ProseMirrorNode, from: number, to: number): boolean {
  if (doc.resolve(from).parent.type.spec.code) return true;
  let code = false;
  doc.nodesBetween(from, to, (node) => {
    if (node.marks.some((mark) => mark.type.spec.code)) code = true;
  });
  return code;
}

/**
 * The pasted address of a card or a page of the workspace is put as the text
 * it is, and becomes a reference where it is then, once it is known that the
 * reader may see what it names; the text is followed through what is typed
 * meanwhile, and stays text if it was changed, was made code, or the reader
 * may not see it. A change of a collaborator that rewrites the block it is in
 * leaves it text too: the text is still the address, and a link.
 */
function pastedAddresses(name: string, options: ReferenceOptions): Plugin<PastedAddress[]> {
  let pastes = 0;
  const settle = (view: EditorView, pasted: PastedAddress, seen: boolean): void => {
    const now = pasteKey.getState(view.state)?.find((each) => each.key === pasted.key);
    if (!now) return;
    const tr = view.state.tr.setMeta(pasteKey, { done: now.key } satisfies PasteMeta);
    const { schema, doc } = view.state;
    const whole =
      now.from < now.to &&
      now.to <= doc.content.size &&
      doc.textBetween(now.from, now.to) === now.text &&
      !codeBetween(doc, now.from, now.to);
    if (seen && whole) {
      tr.replaceWith(now.from, now.to, [schema.node(name, { id: now.id }), schema.text(' ')]);
    }
    view.dispatch(tr);
  };
  return new Plugin<PastedAddress[]>({
    key: pasteKey,
    state: {
      init: () => [],
      apply(tr: Transaction, held: PastedAddress[]): PastedAddress[] {
        const meta = tr.getMeta(pasteKey) as PasteMeta | undefined;
        let next = held;
        if (tr.docChanged) {
          next = next.map((each) => ({
            ...each,
            from: tr.mapping.map(each.from, 1),
            to: tr.mapping.map(each.to, -1),
          }));
        }
        if (meta && 'add' in meta) next = [...next, meta.add];
        if (meta && 'done' in meta) next = next.filter((each) => each.key !== meta.done);
        return next;
      },
    },
    props: {
      handlePaste: (view, event) => {
        const text = event.clipboardData?.getData('text/plain').trim() ?? '';
        // Over a selection an address is the link of what is selected; in code it is text.
        const free = view.state.selection.empty && !inCode(view.state);
        const id = !free || text === '' || /\s/.test(text) ? undefined : options.named(text);
        if (id === undefined || !isBlockId(id)) return copiedCardLink(view, event, text);
        const from = view.state.selection.from;
        // Pasted with no clipboard of its own, it does not come here again.
        view.pasteText(text);
        const to = view.state.selection.from;
        pastes += 1;
        const pasted: PastedAddress = { key: pastes, from, to, text, id };
        view.dispatch(view.state.tr.setMeta(pasteKey, { add: pasted } satisfies PasteMeta));
        void options
          .seen(id)
          .catch(() => false)
          .then((seen) => {
            if (!view.isDestroyed) settle(view, pasted, seen);
          });
        return true;
      },
    },
  });
}
