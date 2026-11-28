/**
 * Toggles and callouts in the editor of a body: a line made a toggle and
 * back, the keys in and around one, that opening and closing one changes no
 * document and writes nothing to the shared one, that it stays open while
 * others write, what pasting one makes, and a callout's tone. Runs a real
 * editor in a simulated page, without layout.
 */
import { Editor } from '@tiptap/core';
import { Collaboration } from '@tiptap/extension-collaboration';
import { TextSelection } from '@tiptap/pm/state';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import { isOpenAt } from '$lib/editor/details-open.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { renderBody } from '$lib/editor/render.ts';
import { toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';
import { documentOf } from '$lib/state/memory/room-docs.ts';

import { bullets, callout, doc, item, paragraph, text, toggle } from '../support/body-docs.ts';

const editors: Editor[] = [];

function create(content: DocNode, shared?: Y.Doc, editable = true): Editor {
  const made = new Editor({
    element: document.createElement('div'),
    editable,
    extensions: [
      ...(shared ? [Collaboration.configure({ document: shared, field: BODY_FIELD })] : []),
      ...bodyExtensions({ history: !shared }),
    ],
    ...(shared ? {} : { content }),
  });
  editors.push(made);
  return made;
}

afterEach(() => {
  for (const made of editors.splice(0)) made.destroy();
});

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);

function caret(editor: Editor, at: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
}

/** A key pressed in the editor, as the page hands it over; whether anything took it. */
function press(editor: Editor, key: string): boolean {
  const taken = editor.view.someProp('handleKeyDown', (handle) =>
    handle(editor.view, new KeyboardEvent('keydown', { key })),
  );
  return taken === true;
}

/** The position just inside the text `words`, at its start or (with `end`) its end. */
function at(editor: Editor, words: string, end = false): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found < 0 && node.isText && node.text?.includes(words))
      found = pos + (node.text.indexOf(words) + (end ? words.length : 0));
    return found < 0;
  });
  if (found < 0) throw new Error(`no ${words}`);
  return found;
}

/** A paste of HTML, as the page hands it over (it has no clipboard of its own). */
function pasteHTML(editor: Editor, html: string): void {
  const data: Record<string, string> = { 'text/html': html, 'text/plain': '' };
  const event = new Event('paste', { bubbles: true, cancelable: true });
  const clipboardData = {
    getData: (type: string) => data[type] ?? '',
    types: Object.keys(data),
    files: [],
    items: [],
  };
  Object.defineProperty(event, 'clipboardData', { value: clipboardData });
  editor.view.dom.dispatchEvent(event);
}

const button = (editor: Editor, index = 0): HTMLButtonElement => {
  const found = editor.view.dom.querySelectorAll<HTMLButtonElement>('.doc-details-button')[index];
  if (!found) throw new Error('no chevron');
  return found;
};

/** A question and its answer, folded away. */
const question = (): DocNode => doc(toggle([text('why')], paragraph(text('because'))));

describe('a toggle in the editor', () => {
  it('is made of the line of the caret, open, and is lines again', () => {
    const editor = create(doc(paragraph(text('why'))));
    caret(editor, at(editor, 'why', true));
    expect(editor.commands.setDetails()).toBe(true);
    expect(stored(editor)).toEqual(doc(toggle([text('why')], paragraph())));
    expect(isOpenAt(editor.state, 0)).toBe(true);
    expect(editor.state.selection.from).toBe(at(editor, 'why', true));
    expect(editor.commands.unsetDetails()).toBe(true);
    expect(stored(editor)).toEqual(doc(paragraph(text('why'))));
    // Not in a list entry, where none may stand.
    const listed = create(doc(bullets(item(paragraph(text('one'))))));
    caret(listed, at(listed, 'one', true));
    expect(listed.can().setDetails()).toBe(false);
  });

  it('opens and closes by its chevron without a change of the document, nor a word to the shared one', async () => {
    const shared = documentOf(question());
    const editor = create(doc(), shared);
    await vi.waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe('details');
    });
    const updates: Uint8Array[] = [];
    shared.on('update', (update: Uint8Array) => updates.push(update));
    const changed: boolean[] = [];
    editor.on('transaction', ({ transaction }) => changed.push(transaction.docChanged));
    const chevron = button(editor);
    const toggled = editor.view.dom.querySelector('[data-details]');
    expect(chevron.getAttribute('aria-expanded')).toBe('false');
    expect(toggled?.hasAttribute('data-open')).toBe(false);

    chevron.click();
    expect(isOpenAt(editor.state, 0)).toBe(true);
    expect(chevron.getAttribute('aria-expanded')).toBe('true');
    expect(toggled?.hasAttribute('data-open')).toBe(true);
    chevron.click();
    expect(isOpenAt(editor.state, 0)).toBe(false);
    expect(chevron.getAttribute('aria-expanded')).toBe('false');

    expect(changed.length).toBeGreaterThanOrEqual(2);
    expect(changed.every((docChanged) => !docChanged)).toBe(true);
    expect(updates).toEqual([]);
    const written: string = shared.getXmlFragment(BODY_FIELD).toJSON();
    expect(written).not.toContain('open');
  });

  it('opens when the caret goes into what it hides, and closing it takes the caret out', () => {
    const editor = create(question());
    expect(isOpenAt(editor.state, 0)).toBe(false);
    caret(editor, at(editor, 'because'));
    expect(isOpenAt(editor.state, 0)).toBe(true);
    button(editor).click();
    expect(isOpenAt(editor.state, 0)).toBe(false);
    expect(editor.state.selection.from).toBe(at(editor, 'why', true));
  });

  it('stays open while another writes before it and in it, at once or one after the other', async () => {
    const shared = documentOf(question());
    const editor = create(doc(), shared);
    await vi.waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe('details');
    });
    button(editor).click();
    /** What another person writes, given as the room gives it. */
    const other = (write: (fragment: Y.XmlFragment) => void): void => {
      const theirs = new Y.Doc();
      Y.applyUpdate(theirs, Y.encodeStateAsUpdate(shared));
      const before = Y.encodeStateVector(theirs);
      write(theirs.getXmlFragment(BODY_FIELD));
      Y.applyUpdate(shared, Y.encodeStateAsUpdate(theirs, before), 'room');
    };
    const line = (words: string): Y.XmlElement => {
      const made = new Y.XmlElement('paragraph');
      made.insert(0, [new Y.XmlText(words)]);
      return made;
    };
    const hiddenText = (fragment: Y.XmlFragment): Y.XmlText => {
      const toggled = fragment.get(fragment.length - 1) as Y.XmlElement;
      const content = toggled.get(1) as Y.XmlElement;
      return (content.get(0) as Y.XmlElement).get(0) as Y.XmlText;
    };
    const toggleAt = (): number =>
      editor.state.doc.content.size - (editor.state.doc.lastChild?.nodeSize ?? 0);

    other((fragment) => {
      fragment.insert(0, [line('first')]);
    });
    other((fragment) => {
      hiddenText(fragment).insert(7, ' so');
    });
    await vi.waitFor(() => {
      expect(editor.state.doc.textContent).toBe('firstwhybecause so');
    });
    expect(isOpenAt(editor.state, toggleAt())).toBe(true);

    // Both in one change: what really changed reaches into the toggle.
    other((fragment) => {
      fragment.insert(0, [line('zeroth')]);
      hiddenText(fragment).insert(10, ', then');
    });
    await vi.waitFor(() => {
      expect(editor.state.doc.textContent).toBe('zerothfirstwhybecause so, then');
    });
    expect(isOpenAt(editor.state, toggleAt())).toBe(true);
  });

  it('is gone into, left and made lines again with Enter and Backspace', () => {
    const editor = create(doc(toggle([text('why it is')], paragraph(text('because')))));
    // Enter in the summary: the rest of it is the first line of what it hides, open.
    caret(editor, at(editor, ' it is'));
    expect(press(editor, 'Enter')).toBe(true);
    expect(stored(editor)).toEqual(
      doc(toggle([text('why')], paragraph(text(' it is')), paragraph(text('because')))),
    );
    expect(isOpenAt(editor.state, 0)).toBe(true);
    expect(editor.state.selection.from).toBe(at(editor, ' it is'));
    // Backspace at the start of what it hides: back to the end of the summary.
    expect(press(editor, 'Backspace')).toBe(true);
    expect(editor.state.selection.from).toBe(at(editor, 'why', true));

    // Enter on an empty last line: the line leaves the toggle.
    caret(editor, at(editor, 'because', true));
    expect(press(editor, 'Enter')).toBe(true);
    expect(press(editor, 'Enter')).toBe(true);
    const after = stored(editor);
    expect(after.content?.map((block) => block.type)).toEqual(['details', 'paragraph']);
    expect(after.content?.[0]?.content?.[1]?.content).toHaveLength(2);
    expect(editor.state.selection.$from.parent.type.name).toBe('paragraph');
    expect(editor.state.selection.$from.depth).toBe(1);

    // Backspace at the start of the summary: lines again.
    caret(editor, at(editor, 'why'));
    expect(press(editor, 'Backspace')).toBe(true);
    expect(stored(editor).content?.map((block) => block.type)).toEqual([
      'paragraph',
      'paragraph',
      'paragraph',
      'paragraph',
    ]);
    expect(validateDoc(stored(editor))).toEqual([]);
  });

  it('takes a new line the page is asked for, as Chrome on Android asks, as Enter', () => {
    const newLine = (): InputEvent =>
      new InputEvent('beforeinput', {
        inputType: 'insertParagraph',
        bubbles: true,
        cancelable: true,
      });
    const editor = create(doc(toggle([text('why it is')], paragraph(text('because')))));
    caret(editor, at(editor, ' it is'));
    // Not on other pages: on iOS ProseMirror takes the Enter itself.
    const iPhone = newLine();
    editor.view.dom.dispatchEvent(iPhone);
    expect(iPhone.defaultPrevented).toBe(false);
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(
      'Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36',
    );
    const event = newLine();
    editor.view.dom.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(stored(editor)).toEqual(
      doc(toggle([text('why')], paragraph(text(' it is')), paragraph(text('because')))),
    );
    // Elsewhere it is the page's, and ProseMirror's.
    caret(editor, at(editor, 'because', true));
    const elsewhere = newLine();
    editor.view.dom.dispatchEvent(elsewhere);
    expect(elsewhere.defaultPrevented).toBe(false);
    vi.restoreAllMocks();
  });

  it('is pasted from a page as a toggle, and copied out and in again the same', () => {
    const editor = create(doc(paragraph()));
    caret(editor, 1);
    pasteHTML(
      editor,
      '<details open><summary>Q <b>here</b></summary><p>A</p><table><tr><td>x</td></tr></table></details>',
    );
    const pasted = stored(editor);
    expect(pasted.content?.[0]?.type).toBe('details');
    expect(pasted.content?.[0]?.content?.[0]).toEqual({
      type: 'detailsSummary',
      content: [text('Q '), text('here', 'bold')],
    });
    expect(pasted.content?.[0]?.content?.[1]?.content?.map((block) => block.type)).toEqual([
      'paragraph',
      'table',
    ]);
    expect(validateDoc(pasted)).toEqual([]);

    const copied = create(question()).getHTML();
    expect(copied).not.toContain('open');
    const again = create(doc(paragraph()));
    again.commands.setContent(copied);
    expect(stored(again)).toEqual(question());
  });
});

describe('a toggle and a callout where a body is only read', () => {
  it('are a closed details and a callout whose icon is a picture of its tone', () => {
    const rendered = renderBody(
      doc(question().content?.[0] ?? paragraph(), callout('warn', paragraph(text('mind')))),
    );
    if (!rendered.ok) throw new Error(rendered.problems.join());
    const details = rendered.fragment.querySelector('details');
    expect(details?.open).toBe(false);
    expect(details?.querySelector('summary')?.textContent).toBe('why');
    const icon = rendered.fragment.querySelector('[data-callout] .doc-callout-icon');
    expect(icon?.getAttribute('role')).toBe('img');
    expect(icon?.getAttribute('aria-label')).toBe('Warning');
    expect(rendered.fragment.querySelector('[data-callout]')?.getAttribute('data-tone')).toBe(
      'warn',
    );
  });
});

describe('a callout in the editor', () => {
  const tip = (): DocNode => doc(callout('tip', paragraph(text('mind'))));
  const iconOf = (editor: Editor): HTMLButtonElement => {
    const found = editor.view.dom.querySelector<HTMLButtonElement>('.doc-callout-icon');
    if (!found) throw new Error('no icon');
    return found;
  };
  const tones = (editor: Editor): HTMLElement | null =>
    editor.view.dom.querySelector<HTMLElement>('.doc-callout-tones');

  it('changes its tone by its icon, as one change of the document', () => {
    const editor = create(tip());
    const icon = iconOf(editor);
    expect(icon.getAttribute('aria-label')).toBe('Callout: Tip. Change its kind');
    icon.click();
    expect(tones(editor)?.hidden).toBe(false);
    expect(icon.getAttribute('aria-expanded')).toBe('true');
    const danger = tones(editor)?.querySelector<HTMLButtonElement>('[data-tone="danger"]');
    expect(tones(editor)?.querySelector('[aria-pressed="true"]')?.getAttribute('data-tone')).toBe(
      'tip',
    );
    danger?.click();
    expect(stored(editor)).toEqual(doc(callout('danger', paragraph(text('mind')))));
    expect(tones(editor)?.hidden).toBe(true);
    expect(editor.view.dom.querySelector('[data-callout]')?.getAttribute('data-tone')).toBe(
      'danger',
    );
    expect(editor.commands.undo()).toBe(true);
    expect(stored(editor)).toEqual(tip());
  });

  it('keeps its tone where the body cannot be changed, its icon a picture that names it', () => {
    const editor = create(tip(), undefined, false);
    const picture = iconOf(editor);
    expect(picture.tagName).toBe('SPAN');
    expect(picture.getAttribute('role')).toBe('img');
    expect(picture.getAttribute('aria-label')).toBe('Tip');
    expect(picture.hasAttribute('aria-haspopup')).toBe(false);
    expect(picture.hasAttribute('aria-expanded')).toBe(false);
    picture.click();
    expect(tones(editor)?.hidden).toBe(true);
    expect(stored(editor)).toEqual(tip());
    // Made writable, the icon is the button again; made read-only, the picture.
    editor.setEditable(true);
    expect(iconOf(editor).tagName).toBe('BUTTON');
    iconOf(editor).click();
    expect(tones(editor)?.hidden).toBe(false);
    editor.setEditable(false);
    expect(iconOf(editor).tagName).toBe('SPAN');
    expect(tones(editor)?.hidden).toBe(true);
  });

  it('is made of the blocks of the selection, and only where it may stand', () => {
    const editor = create(doc(paragraph(text('mind'))));
    caret(editor, at(editor, 'mind'));
    expect(editor.commands.toggleCallout()).toBe(true);
    expect(stored(editor)).toEqual(doc(callout('info', paragraph(text('mind')))));
    const listed = create(doc(bullets(item(paragraph(text('one'))))));
    caret(listed, at(listed, 'one'));
    expect(listed.can().toggleCallout()).toBe(false);
  });
});
