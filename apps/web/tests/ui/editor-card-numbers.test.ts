/**
 * `#` in the editor: where it begins what names a card of the same board (a
 * number or the words of a title), where it is only text (`# ` of a heading,
 * `C#`, an address's `#part`, code), and that what is chosen is put as a
 * reference to the card, by its identifier: the number is never the
 * document's.
 */
import { Editor } from '@tiptap/core';
import { Selection, TextSelection } from '@tiptap/pm/state';
import type { SuggestionProps } from '@tiptap/suggestion';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { SuggestionEntry } from '$lib/editor/extensions.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { cardNumberMatch, type ReferenceOptions } from '$lib/editor/reference.ts';
import { toStoredDoc, type DocNode } from '$lib/editor/schema.ts';
import { COPIED_CARD_LINK } from '$lib/logic/card-numbers.ts';

import { doc, paragraph, text } from '../support/body-docs.ts';

const CARD = '01930000-0000-7000-8000-00000000c003';

let editor: Editor | undefined;

function create(content: DocNode, given?: Partial<ReferenceOptions>): Editor {
  const host = document.body.appendChild(document.createElement('div'));
  editor = new Editor({
    element: host,
    extensions: bodyExtensions({ ...(given ? { reference: given } : {}) }),
    content,
  });
  return editor;
}

/** Types at the end of the document and gives what is being typed after `#` there. */
function typed(target: Editor, value: string): ReturnType<typeof cardNumberMatch> {
  const end = Selection.atEnd(target.state.doc);
  target.view.dispatch(target.state.tr.setSelection(end).insertText(value, end.from));
  return cardNumberMatch({
    char: '#',
    allowSpaces: true,
    allowToIncludeChar: false,
    allowedPrefixes: null,
    startOfLine: false,
    $position: target.state.selection.$from,
  });
}

/** Types a letter as a keyboard does: the editor's input rules see it. */
function keyed(target: Editor, letter: string): void {
  const { view } = target;
  const { from, to } = view.state.selection;
  const insert = () => view.state.tr.insertText(letter, from, to);
  const taken = view.someProp('handleTextInput', (handle) =>
    handle(view, from, to, letter, insert),
  );
  if (!taken) view.dispatch(insert());
}

const stored = (target: Editor): DocNode => toStoredDoc(target.getJSON() as DocNode);

afterEach(() => {
  vi.unstubAllGlobals();
  editor?.destroy();
  editor = undefined;
  document.body.replaceChildren();
});

describe('what is typed after `#`', () => {
  it('begins at the start of a line, after a space or a bracket, and holds the words of a title', () => {
    const made = create(doc(paragraph()));
    expect(typed(made, '#')).toBeNull();
    expect(typed(made, '3')).toMatchObject({ query: '3', text: '#3' });
    const match = typed(made, '두 단어');
    expect(match).toMatchObject({ query: '3두 단어', text: '#3두 단어' });
    expect(made.state.doc.textBetween(match?.range.from ?? 0, match?.range.to ?? 0)).toBe(
      '#3두 단어',
    );
    const spaced = create(doc(paragraph(text('See #'))));
    expect(typed(spaced, '12')).toMatchObject({ query: '12' });
    const bracketed = create(doc(paragraph(text('(#'))));
    expect(typed(bracketed, 'plan')).toMatchObject({ query: 'plan' });
  });

  it('is nothing where `#` is part of a word, an address, a heading or `[[`', () => {
    for (const before of [
      'C#',
      'page#',
      'https://example.com/#',
      '##',
      '[[#',
      '#3]',
      '#a#',
      // `[[` is taking the words: one list at a time.
      '[[Bug #',
    ]) {
      const made = create(doc(paragraph(text(before))));
      expect(typed(made, '1'), before).toBeNull();
    }
    // `# ` is a heading: what follows `#` begins with no space.
    const heading = create(doc(paragraph()));
    heading.commands.focus('end');
    for (const letter of ['#', ' ']) keyed(heading, letter);
    expect(heading.getJSON().content[0]?.type).toBe('heading');
    const spaced = create(doc(paragraph(text('x '))));
    expect(typed(spaced, '# plan')).toBeNull();
  });

  it('is nothing in code', () => {
    const block = create(doc({ type: 'codeBlock', content: [text('a ')] }));
    expect(typed(block, '#3')).toBeNull();
    const marked = create(doc(paragraph(text('x #3', 'code'))));
    const $position = marked.state.doc.resolve(Selection.atEnd(marked.state.doc).from);
    expect(
      cardNumberMatch({
        char: '#',
        allowSpaces: true,
        allowToIncludeChar: false,
        allowedPrefixes: null,
        startOfLine: false,
        $position,
      }),
    ).toBeNull();
  });
});

describe('a card chosen after `#`', () => {
  it('is offered from what the board names, and put as a reference to it alone', async () => {
    const asked: string[] = [];
    let offer: SuggestionProps<SuggestionEntry, SuggestionEntry> | undefined;
    const made = create(doc(paragraph(text('See '))), {
      numbered: (query) => {
        asked.push(query);
        return query === '3' ? [{ value: CARD, label: '#3 Plan' }] : [];
      },
      renderNumbered: () => ({
        onStart: (props) => {
          offer = props;
        },
        onUpdate: (props) => {
          offer = props;
        },
      }),
    });
    made.commands.focus('end');
    made.commands.insertContent('#3');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(asked).toContain('3');
    expect(offer?.items).toEqual([{ value: CARD, label: '#3 Plan' }]);
    const [entry] = offer?.items ?? [];
    if (!offer || !entry) throw new Error('nothing was offered');
    offer.command(entry);
    expect(stored(made)).toEqual(
      doc(paragraph(text('See '), { type: 'reference', attrs: { id: CARD } }, text(' '))),
    );
    expect(JSON.stringify(stored(made))).not.toContain('#3');
  });

  it('offers nothing without a board', async () => {
    let offered: SuggestionEntry[] | undefined;
    const made = create(doc(paragraph()), {
      renderNumbered: () => ({
        onStart: (props) => {
          offered = props.items;
        },
      }),
    });
    made.commands.focus('end');
    made.commands.insertContent('#3');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(offered ?? []).toEqual([]);
  });
});

const ADDRESS = `https://app.example/w/ws/boards/b/v/${CARD}`;

/** Pastes a card's link as the app copies it: its address as text, and as HTML under its number and title. */
function pasteCopiedLink(target: Editor): boolean {
  const html = `<a href="${ADDRESS}" ${COPIED_CARD_LINK}="">#12 Plan</a>`;
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: (type: string) => string };
  };
  event.clipboardData = {
    files: [],
    getData: (type) => (type === 'text/plain' ? ADDRESS : type === 'text/html' ? html : ''),
  };
  return Boolean(
    target.view.someProp('handlePaste', (handle) =>
      handle(target.view, event as unknown as ClipboardEvent, target.state.doc.slice(0, 0)),
    ),
  );
}

describe('a card’s link copied in the app, pasted where it is not made a reference', () => {
  it('over a selection, links what is selected to the address, and puts none of its words', () => {
    // The simulated page has no clipboard events; the editor makes one to paste text with.
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    const made = create(doc(paragraph(text('See here'))), { named: () => CARD });
    made.view.dispatch(made.state.tr.setSelection(TextSelection.create(made.state.doc, 5, 9)));
    expect(pasteCopiedLink(made)).toBe(true);
    expect(made.getText()).toBe('See here');
    const [, linked] = stored(made).content?.[0]?.content ?? [];
    expect(linked).toMatchObject({ type: 'text', text: 'here' });
    expect(linked?.marks).toHaveLength(1);
    expect(linked?.marks?.[0]).toMatchObject({ type: 'link', attrs: { href: ADDRESS } });
  });

  it('naming no card here, is its address as text', () => {
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    const made = create(doc(paragraph(text('See '))), { named: () => undefined });
    made.commands.focus('end');
    expect(pasteCopiedLink(made)).toBe(true);
    expect(made.getText()).toBe(`See ${ADDRESS}`);
    expect(JSON.stringify(stored(made))).not.toContain('#12');
  });
});
