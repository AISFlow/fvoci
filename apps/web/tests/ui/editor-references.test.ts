/**
 * References in the editor and in a body that is shown: the node draws what
 * the caller knows of what it names and holds the identifier alone, `[[`
 * is followed across the spaces of a title, a pasted address becomes a
 * reference only for what the reader may see, and both components draw again
 * when what is known changes.
 */
import { Editor } from '@tiptap/core';
import { Selection, TextSelection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { BodyReferences } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import BodyView from '$lib/editor/BodyView.svelte';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import type { ShownReference } from '$lib/editor/reference-chip.ts';
import { referenceMatch, type ReferenceOptions } from '$lib/editor/reference.ts';
import { plainText } from '$lib/editor/body-text.ts';
import { toStoredDoc, type DocNode } from '$lib/editor/schema.ts';

import { doc, paragraph, reference, text } from '../support/body-docs.ts';

const SEEN = '01930000-0000-7000-8000-00000000b001';
const UNSEEN = '01930000-0000-7000-8000-00000000b002';
const ADDRESS = `https://app.example/w/ws/boards/b/v/${SEEN}`;
const HIDDEN_ADDRESS = `https://app.example/w/ws/boards/b/v/${UNSEEN}`;

const plan: ShownReference = { title: 'Plan', icon: '🚀', kind: 'card', href: '/card/plan' };

function references(known: Record<string, ShownReference | null | undefined>): BodyReferences {
  return {
    shown: (id) => known[id],
    search: () => [],
    named: (address) => address.split('/v/')[1],
    seen: (id) => Promise.resolve(Boolean(known[id])),
  };
}

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

const stored = (target: Editor): DocNode => toStoredDoc(target.getJSON() as DocNode);
const chips = (root: ParentNode): HTMLElement[] =>
  Array.from(root.querySelectorAll<HTMLElement>('span[data-reference]'));

/** Types at the end of the document and gives what is being typed after `[[` there. */
function typed(target: Editor, value: string): ReturnType<typeof referenceMatch> {
  const end = Selection.atEnd(target.state.doc);
  target.view.dispatch(target.state.tr.setSelection(end).insertText(value, end.from));
  const $position = target.state.selection.$from;
  return referenceMatch({
    char: '[[',
    allowSpaces: true,
    allowToIncludeChar: false,
    allowedPrefixes: null,
    startOfLine: false,
    $position,
  });
}

/** Pastes text the way a browser reports it; whether the editor took it as its own. */
function paste(target: Editor, value: string): boolean {
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: (type: string) => string };
  };
  event.clipboardData = { files: [], getData: (type) => (type === 'text/plain' ? value : '') };
  return Boolean(
    target.view.someProp('handlePaste', (handle) =>
      handle(target.view, event as unknown as ClipboardEvent, target.state.doc.slice(0, 0)),
    ),
  );
}

const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  vi.unstubAllGlobals();
  editor?.destroy();
  editor = undefined;
  document.body.replaceChildren();
});

describe('a reference in the editor', () => {
  it('is drawn from what the caller knows, and is the identifier alone in the document', () => {
    const known = references({ [SEEN]: plan, [UNSEEN]: null });
    const made = create(doc(paragraph(text('See '), reference(SEEN), reference(UNSEEN))), {
      drawing: () => known,
    });
    const [seen, unseen] = chips(made.view.dom);
    expect(seen?.dataset.state).toBe('shown');
    expect(seen?.querySelector('a')?.getAttribute('href')).toBe('/card/plan');
    expect(seen?.textContent).toBe('🚀Plan');
    expect(unseen?.dataset.state).toBe('unseen');
    expect(unseen?.textContent).toBe("Link you can't see");
    expect(unseen?.querySelector('a')).toBeNull();

    // What is copied out, stored, and read as text has no title in it.
    expect(made.getHTML()).not.toContain('Plan');
    expect(made.getHTML()).toContain(`data-block-id="${SEEN}"`);
    expect(JSON.stringify(stored(made))).not.toContain('Plan');
    expect(made.getText()).toBe('See ');
    expect(plainText(stored(made))).toBe('See ');
  });

  it('shows no title where the caller hands nothing to draw from', () => {
    const made = create(doc(paragraph(reference(SEEN))));
    expect(chips(made.view.dom).map((chip) => chip.dataset.state)).toEqual(['unseen']);
  });

  it('opens what a plain click on its link names', () => {
    const open = vi.fn();
    const known = { ...references({ [SEEN]: plan }), open };
    const made = create(doc(paragraph(reference(SEEN))), { drawing: () => known });
    const link = chips(made.view.dom)[0]?.querySelector('a');
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    link?.dispatchEvent(click);
    expect(open).toHaveBeenCalledExactlyOnceWith(SEEN);
    expect(click.defaultPrevented).toBe(true);
  });
});

describe('what is typed after `[[`', () => {
  it('is followed across the spaces of a title, up to the cursor', () => {
    const made = create(doc(paragraph(text('Before '))));
    expect(typed(made, '[')).toBeNull();
    expect(typed(made, '[')).toMatchObject({ query: '', text: '[[' });
    const match = typed(made, 'Road map 2');
    expect(match).toMatchObject({ query: 'Road map 2', text: '[[Road map 2' });
    // The range is the brackets and the title: what a choice replaces.
    expect(made.state.doc.textBetween(match?.range.from ?? 0, match?.range.to ?? 0)).toBe(
      '[[Road map 2',
    );
  });

  it('ends at a closing bracket, at a reference, and past the longest title looked for', () => {
    const closed = create(doc(paragraph(text('[[done]'))));
    expect(typed(closed, ' and more')).toBeNull();
    closed.destroy();
    const past = create(doc(paragraph(text('[[one '), reference(SEEN))));
    expect(typed(past, ' two')).toBeNull();
    past.destroy();
    const long = create(doc(paragraph(text('[['))));
    expect(typed(long, 'x'.repeat(80))).not.toBeNull();
    expect(typed(long, 'x')).toBeNull();
    long.destroy();
  });

  it('is nothing in code', () => {
    const block = create(doc({ type: 'codeBlock', content: [text('a = b[[0')] }));
    expect(typed(block, ']')).toBeNull();
    expect(typed(block, ' [[x')).toBeNull();
    block.destroy();
    const marked = create(doc(paragraph({ ...text('[[x'), marks: [{ type: 'code' }] })));
    const $position = marked.state.doc.resolve(Selection.atEnd(marked.state.doc).from);
    expect(
      referenceMatch({
        char: '[[',
        allowSpaces: true,
        allowToIncludeChar: false,
        allowedPrefixes: null,
        startOfLine: false,
        $position,
      }),
    ).toBeNull();
  });
});

describe('a pasted address', () => {
  const given = (known: BodyReferences): Partial<ReferenceOptions> => ({
    drawing: () => known,
    named: known.named,
    seen: known.seen,
  });

  it('of what the reader may see becomes a reference to it', async () => {
    // The simulated page has no clipboard events; the editor makes one to paste text with.
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    const made = create(doc(paragraph(text('See '))), given(references({ [SEEN]: plan })));
    made.commands.focus('end');
    expect(paste(made, ADDRESS)).toBe(true);
    await settle();
    expect(stored(made).content?.[0]?.content).toEqual([text('See '), reference(SEEN), text(' ')]);
    expect(chips(made.view.dom)[0]?.textContent).toBe('🚀Plan');
  });

  it('of what the reader may not see stays the address it was', async () => {
    // The simulated page has no clipboard events; the editor makes one to paste text with.
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    const made = create(doc(paragraph(text('See '))), given(references({ [UNSEEN]: null })));
    made.commands.focus('end');
    expect(paste(made, HIDDEN_ADDRESS)).toBe(true);
    await settle();
    expect(chips(made.view.dom)).toHaveLength(0);
    expect(made.getText()).toBe(`See ${HIDDEN_ADDRESS}`);
  });

  it('becomes a reference where it was pasted, whatever is typed before the answer', async () => {
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    let answer: (seen: boolean) => void = () => undefined;
    const known = references({ [SEEN]: plan });
    const made = create(doc(paragraph(text('See '))), {
      ...given(known),
      seen: () => new Promise((resolve) => (answer = resolve)),
    });
    // The caret at the end, without the focus (and its scrolling) the simulated page lacks.
    made.view.dispatch(made.state.tr.setSelection(Selection.atEnd(made.state.doc)));
    expect(paste(made, ADDRESS)).toBe(true);
    // Typed before it, and the caret taken elsewhere: the reference is put where the address is.
    made.view.dispatch(made.state.tr.insertText('Now ', 1));
    made.view.dispatch(made.state.tr.setSelection(TextSelection.create(made.state.doc, 2)));
    answer(true);
    await settle();
    expect(stored(made).content?.[0]?.content).toEqual([
      text('Now See '),
      reference(SEEN),
      text(' '),
    ]);
    expect(made.state.selection.from).toBe(2);
  });

  it('becomes a reference each time the same address is pasted, also while another waits', async () => {
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    const answers: ((seen: boolean) => void)[] = [];
    const made = create(doc(paragraph(text('See '))), {
      ...given(references({ [SEEN]: plan })),
      seen: () => new Promise((resolve) => answers.push(resolve)),
    });
    const atEnd = () => {
      made.view.dispatch(made.state.tr.setSelection(Selection.atEnd(made.state.doc)));
    };
    atEnd();
    expect(paste(made, ADDRESS)).toBe(true);
    answers.shift()?.(true);
    await settle();
    atEnd();
    expect(paste(made, ADDRESS)).toBe(true);
    atEnd();
    expect(paste(made, ADDRESS)).toBe(true);
    for (const answer of answers.splice(0).reverse()) answer(true);
    await settle();
    expect(stored(made).content?.[0]?.content).toEqual([
      text('See '),
      reference(SEEN),
      text(' '),
      reference(SEEN),
      text(' '),
      reference(SEEN),
      text(' '),
    ]);
  });

  it('stays the address it was if it was changed before the answer', async () => {
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    let answer: (seen: boolean) => void = () => undefined;
    const made = create(doc(paragraph(text('See '))), {
      ...given(references({ [SEEN]: plan })),
      seen: () => new Promise((resolve) => (answer = resolve)),
    });
    // The caret at the end, without the focus (and its scrolling) the simulated page lacks.
    made.view.dispatch(made.state.tr.setSelection(Selection.atEnd(made.state.doc)));
    expect(paste(made, ADDRESS)).toBe(true);
    const end = made.state.doc.content.size - 1;
    made.view.dispatch(made.state.tr.delete(end - 2, end));
    answer(true);
    await settle();
    expect(chips(made.view.dom)).toHaveLength(0);
    expect(made.getText()).toBe(`See ${ADDRESS.slice(0, -2)}`);
  });

  it('stays the address it was if it was made code before the answer', async () => {
    vi.stubGlobal('ClipboardEvent', class extends Event {});
    let answer: (seen: boolean) => void = () => undefined;
    const made = create(doc(paragraph(text('See '))), {
      ...given(references({ [SEEN]: plan })),
      seen: () => new Promise((resolve) => (answer = resolve)),
    });
    made.view.dispatch(made.state.tr.setSelection(Selection.atEnd(made.state.doc)));
    expect(paste(made, ADDRESS)).toBe(true);
    const { schema, doc: now } = made.state;
    const code = schema.marks.code;
    if (!code) throw new Error('no code mark');
    made.view.dispatch(made.state.tr.addMark(5, now.content.size - 1, code.create()));
    answer(true);
    await settle();
    expect(chips(made.view.dom)).toHaveLength(0);
    expect(made.getText()).toBe(`See ${ADDRESS}`);
  });

  it('is left to the editor over a selection, among other words, and in code', () => {
    const known = references({ [SEEN]: plan });
    const made = create(doc(paragraph(text('See this'))), given(known));
    made.commands.focus('end');
    expect(paste(made, `look at ${ADDRESS}`)).toBe(false);
    expect(paste(made, 'https://elsewhere.example/page')).toBe(false);
    made.commands.selectAll();
    expect(paste(made, ADDRESS)).toBe(false);
    made.destroy();
    const code = create(doc({ type: 'codeBlock', content: [text('x')] }), given(known));
    code.commands.focus('end');
    expect(paste(code, ADDRESS)).toBe(false);
    code.destroy();
    const marked = create(
      doc(paragraph({ ...text('x'), marks: [{ type: 'code' }] })),
      given(known),
    );
    marked.commands.focus('end');
    expect(paste(marked, ADDRESS)).toBe(false);
    marked.destroy();
  });
});

describe('the components', () => {
  it('BodyEditor draws its references, and again when what is known changes', async () => {
    const body = doc(paragraph(reference(SEEN)));
    const { container, rerender } = render(BodyEditor, {
      doc: body,
      references: references({ [SEEN]: undefined }),
    });
    const chip = (): HTMLElement | undefined => chips(container)[0];
    expect(chip()?.dataset.state).toBe('pending');
    await rerender({ doc: body, references: references({ [SEEN]: plan }) });
    expect(chip()?.dataset.state).toBe('shown');
    expect(chip()?.textContent).toBe('🚀Plan');
    await rerender({
      doc: body,
      references: references({ [SEEN]: { ...plan, title: 'Renamed', icon: '' } }),
    });
    expect(chip()?.textContent).toBe('Renamed');
    await rerender({ doc: body, references: references({ [SEEN]: null }) });
    expect(chip()?.dataset.state).toBe('unseen');
    expect(container.textContent).not.toContain('Renamed');
  });

  it('BodyView draws its references, opens the one clicked, and shows none without a caller', async () => {
    const open = vi.fn();
    const body = doc(paragraph(text('See '), reference(SEEN), reference(UNSEEN), reference('')));
    const { container, rerender } = render(BodyView, {
      doc: body,
      references: { ...references({ [SEEN]: plan, [UNSEEN]: null }), open },
    });
    expect(chips(container).map((chip) => chip.dataset.state)).toEqual([
      'shown',
      'unseen',
      'unseen',
    ]);
    chips(container)[0]
      ?.querySelector('a')
      ?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(open).toHaveBeenCalledExactlyOnceWith(SEEN);

    await rerender({ doc: body, references: undefined });
    expect(chips(container).map((chip) => chip.dataset.state)).toEqual([
      'unseen',
      'unseen',
      'unseen',
    ]);
    expect(container.textContent).not.toContain('Plan');
  });
});
