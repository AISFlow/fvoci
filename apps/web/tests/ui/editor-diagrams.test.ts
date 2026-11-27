/**
 * A diagram in the editor: its picture alone until its place is opened, the
 * source right under the picture while it is, and nothing of that in the
 * document. Runs a real editor in a simulated page, without layout; the
 * arrow keys, which need layout, are in e2e/diagram-place.spec.ts.
 */
import { Editor } from '@tiptap/core';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { placeOf } from '$lib/editor/editing-place.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { toStoredDoc, type DocNode } from '$lib/editor/schema.ts';

import { code, doc, paragraph, text } from '../support/body-docs.ts';

type Drawn =
  { ok: true; picture: SVGSVGElement } | { ok: false; reason: 'invalid'; detail?: string };

/** A picture that says what it was drawn from; a source with "bad" in it cannot be drawn. */
function picture(source: string): SVGSVGElement {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('data-source', source);
  return svg;
}
const drawDiagram = vi.fn((source: string): Promise<Drawn> =>
  Promise.resolve(
    source.includes('bad')
      ? { ok: false, reason: 'invalid', detail: 'Parse error on line 2' }
      : { ok: true, picture: picture(source) },
  ),
);
vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: (source: string) => drawDiagram(source),
}));

// Opening a place scrolls it into sight, which asks for the size of text: jsdom has none.
Range.prototype.getClientRects = function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return new DOMRect();
};

let editor: Editor | undefined;
let updates = 0;

function create(content: DocNode, editable = true): Editor {
  const element = document.createElement('div');
  document.body.append(element);
  updates = 0;
  editor = new Editor({
    element,
    injectCSS: false,
    editable,
    extensions: bodyExtensions(),
    content,
    onUpdate: () => {
      updates += 1;
    },
  });
  return editor;
}

const stored = (target: Editor): DocNode => toStoredDoc(target.getJSON() as DocNode);
const block = (target: Editor): HTMLElement => {
  const found = target.view.dom.querySelector<HTMLElement>('.doc-diagram-block');
  if (!found) throw new Error('no diagram');
  return found;
};
const figure = (target: Editor): HTMLElement => {
  const found = block(target).querySelector<HTMLElement>('.doc-diagram');
  if (!found) throw new Error('no picture');
  return found;
};
const isOpen = (target: Editor): boolean => block(target).hasAttribute('data-open');

/** A key pressed in the editor, the way the page hands it over. Whether something took it. */
function press(target: Editor, init: KeyboardEventInit): boolean {
  const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
  target.view.dom.dispatchEvent(event);
  return event.defaultPrevented;
}

/** Types into the editor the way a keyboard does. */
function type(target: Editor, value: string): void {
  for (const character of value) {
    const { from, to } = target.state.selection;
    const insert = () => target.state.tr.insertText(character, from, to);
    const handled = target.view.someProp('handleTextInput', (handle) =>
      handle(target.view, from, to, character, insert),
    );
    if (!handled) target.view.dispatch(insert());
  }
}

const FLOW = 'flowchart LR\n  A --> B';
const BODY = doc(paragraph(text('before')), code('mermaid', FLOW), paragraph(text('after')));
/** Where the diagram of BODY starts. */
const AT = 8;

afterEach(() => {
  editor?.destroy();
  editor = undefined;
  document.body.replaceChildren();
  drawDiagram.mockClear();
});

describe('a closed diagram', () => {
  it('is its picture: the source is in the page but not in sight', async () => {
    const target = create(BODY);
    expect(isOpen(target)).toBe(false);
    await vi.waitFor(() => {
      expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe(FLOW);
    });
    expect(block(target).getAttribute('data-state')).toBe('drawn');
    // Named for what it is and what a press does.
    expect(figure(target).getAttribute('role')).toBe('button');
    expect(figure(target).getAttribute('aria-label')).toBe('Flowchart, press to edit');
    // The source is the block's text, and stays the document.
    expect(block(target).querySelector('pre code')?.textContent).toBe(FLOW);
    expect(stored(target)).toEqual(BODY);
  });

  it('takes the selection as a whole, never the cursor into its source', () => {
    const target = create(BODY);
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, AT + 3)),
    );
    expect(target.state.selection).toBeInstanceOf(NodeSelection);
    expect(target.state.selection.from).toBe(AT);
    expect(placeOf(target.state)).toBeNull();
  });

  it('shows the source and what went wrong in place of a picture it cannot draw', async () => {
    const target = create(doc(code('mermaid', 'flowchart LR\n  bad -->')));
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('error');
    });
    expect(figure(target).querySelector('[role="note"]')?.textContent).toBe(
      'This diagram cannot be drawn.',
    );
    expect(figure(target).querySelector('.doc-diagram-copy')?.textContent).toBe(
      'flowchart LR\n  bad -->',
    );
    expect(figure(target).querySelector('.doc-embed-detail')?.textContent).toBe(
      'Parse error on line 2',
    );
    // The press is its name; what went wrong describes it.
    const described = figure(target).getAttribute('aria-describedby');
    expect(described && document.getElementById(described)?.textContent).toBe(
      'This diagram cannot be drawn.',
    );
  });

  it('is a plain code block when its language is another', () => {
    const target = create(doc(code('ts', 'let a = 1;')));
    expect(target.view.dom.querySelector('.doc-diagram-block')).toBeNull();
    expect(target.view.dom.querySelector('pre > code.language-ts')?.textContent).toBe('let a = 1;');
    target.commands.setContent(doc(code('mermaid', 'pie')));
    expect(target.view.dom.querySelector('.doc-diagram-block')).not.toBeNull();
    target.commands.updateAttributes('codeBlock', { language: 'text' });
    expect(target.view.dom.querySelector('.doc-diagram-block')).toBeNull();
  });
});

describe('the keys beside a closed diagram', () => {
  it('select it with Backspace from the line under it and Delete from the line over it, joining nothing', () => {
    const target = create(BODY);
    const after = AT + (target.state.doc.nodeAt(AT)?.nodeSize ?? 0);
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, after + 1)),
    );
    expect(press(target, { key: 'Backspace' })).toBe(true);
    expect(target.state.selection).toBeInstanceOf(NodeSelection);
    expect(target.state.selection.from).toBe(AT);
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, AT - 1)),
    );
    expect(press(target, { key: 'Delete' })).toBe(true);
    expect(target.state.selection.from).toBe(AT);
    expect(stored(target)).toEqual(BODY);
    expect(isOpen(target)).toBe(false);
  });

  it('take the whole diagram into a selection drawn into it, not part of its hidden source', () => {
    const target = create(BODY);
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, 3, AT + 4)),
    );
    const { selection } = target.state;
    expect(selection.from).toBe(3);
    expect(selection.to).toBeGreaterThan(AT + (target.state.doc.nodeAt(AT)?.nodeSize ?? 0));
  });
});

describe('a selection that reaches a closed diagram at an end of the body', () => {
  // Found by an independent review (its branch `fvoci/review-web-mermaid`).
  it('takes the whole last diagram, with no text behind it to end in', () => {
    const target = create(doc(paragraph(text('before')), code('mermaid', FLOW)));
    const diagramAt = target.state.doc.child(0).nodeSize;
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, 2, diagramAt + 4)),
    );
    expect(placeOf(target.state)).toBeNull();
    expect(target.state.selection.to).toBe(target.state.doc.content.size);
    // What is typed then takes the diagram's place whole, not part of its source.
    type(target, 'x');
    expect(stored(target)).toEqual(doc(paragraph(text('bx'))));
  });

  it('takes in every closed diagram it reaches, two in a row too', () => {
    const target = create(doc(paragraph(text('p')), code('mermaid', FLOW), code('mermaid', FLOW)));
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, 2, 6)),
    );
    expect(target.state.selection.to).toBe(target.state.doc.content.size);
    type(target, 'x');
    expect(stored(target)).toEqual(doc(paragraph(text('px'))));
  });

  it('starting right before the only diagram of the body, takes it in', () => {
    const target = create(doc(code('mermaid', FLOW)));
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, 0, 4)),
    );
    expect(target.state.selection.from).toBe(0);
    expect(target.state.selection.to).toBe(target.state.doc.content.size);
  });

  it('does not leave its other end in a diagram whose place has just closed', () => {
    const target = create(doc(code('mermaid', FLOW), paragraph(text('after'))));
    figure(target).click();
    const { from } = target.state.selection;
    // Drawn from inside the open source into the line under it; the place stays open.
    target.view.dispatch(
      target.state.tr.setSelection(
        TextSelection.create(target.state.doc, from - 3, target.state.doc.content.size - 1),
      ),
    );
    expect(isOpen(target)).toBe(true);
    document.body.click();
    expect(isOpen(target)).toBe(false);
    expect(target.state.selection.from).toBe(0);
    type(target, 'x');
    expect(stored(target)).toEqual(doc(paragraph(text('x'))));
  });

  it('takes the whole first diagram, drawn backwards to it', () => {
    const target = create(doc(code('mermaid', FLOW), paragraph(text('after'))));
    const afterAt = target.state.doc.child(0).nodeSize;
    target.view.dispatch(
      target.state.tr.setSelection(TextSelection.create(target.state.doc, afterAt + 2, 4)),
    );
    expect(placeOf(target.state)).toBeNull();
    expect(target.state.selection.from).toBe(0);
  });
});

describe('opening and closing the place', () => {
  it('opens on a press of the picture, with the cursor at the end of the source', () => {
    const target = create(BODY);
    figure(target).click();
    expect(isOpen(target)).toBe(true);
    expect(placeOf(target.state)).toMatchObject({ kind: 'diagram', pos: AT });
    const { selection } = target.state;
    expect(selection).toBeInstanceOf(TextSelection);
    expect(selection.from).toBe(AT + 1 + FLOW.length);
    // Open, the picture is no button any more.
    expect(figure(target).getAttribute('role')).not.toBe('button');
  });

  it('opens on Enter while selected, closes on Escape and on Ctrl+Enter, back on the block', () => {
    const target = create(BODY);
    target.view.dispatch(target.state.tr.setSelection(NodeSelection.create(target.state.doc, AT)));
    expect(press(target, { key: 'Enter' })).toBe(true);
    expect(isOpen(target)).toBe(true);

    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    const stop = vi.spyOn(escape, 'stopPropagation');
    target.view.dom.dispatchEvent(escape);
    expect(stop).toHaveBeenCalled();
    expect(isOpen(target)).toBe(false);
    expect(target.state.selection).toBeInstanceOf(NodeSelection);
    expect(target.state.selection.from).toBe(AT);

    press(target, { key: 'Enter' });
    expect(isOpen(target)).toBe(true);
    press(target, { key: 'Enter', ctrlKey: true });
    expect(isOpen(target)).toBe(false);
    expect(target.state.selection.from).toBe(AT);
  });

  it('leaves the keys to an input method that is composing', () => {
    const target = create(BODY);
    figure(target).click();
    expect(press(target, { key: 'Escape', isComposing: true })).toBe(false);
    expect(isOpen(target)).toBe(true);
  });

  it('closes on a press elsewhere, not on a touch that only scrolls, and when the cursor leaves the block', () => {
    const target = create(BODY);
    figure(target).click();
    // A finger put down to scroll the page is no press.
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    expect(isOpen(target)).toBe(true);
    document.body.click();
    expect(isOpen(target)).toBe(false);

    figure(target).click();
    // A press inside the place keeps it open.
    block(target).querySelector('pre')?.click();
    expect(isOpen(target)).toBe(true);
    target.view.dispatch(target.state.tr.setSelection(TextSelection.create(target.state.doc, 2)));
    expect(isOpen(target)).toBe(false);
  });

  it('closes with its button, back on the block, and with the keys while the button has the focus', () => {
    const target = create(BODY);
    figure(target).click();
    const done = (): HTMLButtonElement => {
      const found = block(target).querySelector<HTMLButtonElement>('.doc-place-footer button');
      if (!found) throw new Error('no button');
      return found;
    };
    done().click();
    expect(isOpen(target)).toBe(false);
    expect(target.state.selection).toBeInstanceOf(NodeSelection);

    // The editor does not hear keys on the button; the place does, and keeps Escape to itself.
    figure(target).click();
    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    const heard = vi.fn();
    document.addEventListener('keydown', heard);
    done().dispatchEvent(escape);
    document.removeEventListener('keydown', heard);
    expect(isOpen(target)).toBe(false);
    expect(heard).not.toHaveBeenCalled();
    figure(target).click();
    done().dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }),
    );
    expect(isOpen(target)).toBe(false);
  });

  it('does not let typed text take the place of a selected diagram', () => {
    const target = create(BODY);
    target.view.dispatch(target.state.tr.setSelection(NodeSelection.create(target.state.doc, AT)));
    type(target, 'a');
    expect(stored(target)).toEqual(BODY);
  });

  it('changes nothing in the document by opening and closing', () => {
    const target = create(BODY);
    const before = JSON.stringify(target.getJSON());
    for (let round = 0; round < 3; round += 1) {
      figure(target).click();
      press(target, { key: 'Escape' });
    }
    expect(JSON.stringify(target.getJSON())).toBe(before);
    expect(updates).toBe(0);
  });

  it('does not open where the body cannot be edited', () => {
    const target = create(BODY, false);
    figure(target).click();
    target.view.dispatch(target.state.tr.setSelection(NodeSelection.create(target.state.doc, AT)));
    press(target, { key: 'Enter' });
    expect(isOpen(target)).toBe(false);
    expect(figure(target).getAttribute('role')).not.toBe('button');
  });

  it('closes when the editor becomes read-only', async () => {
    const target = create(BODY);
    figure(target).click();
    target.setEditable(false);
    await vi.waitFor(() => {
      expect(isOpen(target)).toBe(false);
    });
    expect(figure(target).getAttribute('role')).not.toBe('button');
  });

  it('starts open for a diagram just made by typing its fence', () => {
    const target = create(doc(paragraph()));
    type(target, '```mermaid ');
    expect(stored(target)).toEqual(doc({ type: 'codeBlock', attrs: { language: 'mermaid' } }));
    expect(isOpen(target)).toBe(true);
    type(target, 'pie');
    expect(stored(target)).toEqual(doc(code('mermaid', 'pie')));
  });

  it('does not open an empty diagram that a new document brings, wherever the cursor lands', () => {
    const target = create(doc(paragraph(text('```'))));
    target.commands.setContent(doc(code('mermaid'), paragraph()));
    target.view.dispatch(target.state.tr.setSelection(TextSelection.create(target.state.doc, 1)));
    expect(isOpen(target)).toBe(false);
    expect(target.state.selection).toBeInstanceOf(NodeSelection);
    expect(figure(target).textContent).toBe('Empty diagram');
  });
});

describe('editing the source', () => {
  it('makes a new line of every Enter, empty lines too, without leaving the source', () => {
    const target = create(BODY);
    figure(target).click();
    const before = block(target);
    for (let line = 0; line < 3; line += 1) {
      expect(press(target, { key: 'Enter' })).toBe(true);
    }
    expect(block(target)).toBe(before);
    type(target, '  B --> C');
    expect(isOpen(target)).toBe(true);
    expect(stored(target)).toEqual(
      doc(
        paragraph(text('before')),
        code('mermaid', `${FLOW}\n\n\n  B --> C`),
        paragraph(text('after')),
      ),
    );
  });

  it('leaves composing Enter to the input method while editing a diagram source', () => {
    const target = create(BODY);
    figure(target).click();
    target.view.dom.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    expect(press(target, { key: 'Enter' })).toBe(false);
    expect(press(target, { key: 'Enter', isComposing: true })).toBe(false);
    target.view.dom.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
    // eslint-disable-next-line @typescript-eslint/no-deprecated -- Safari marks IME-confirming Enter with keyCode 229
    expect(press(target, { key: 'Enter', keyCode: 229 })).toBe(false);
    expect(stored(target)).toEqual(BODY);
  });

  it('redraws for a changed theme when an open diagram view is rebuilt mid-switch', async () => {
    const target = create(BODY);
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('drawn');
    });
    expect(drawDiagram).toHaveBeenCalledTimes(1);
    document.documentElement.classList.add('dark');
    try {
      // Opening while the dark redraw is still pending must keep the light look recorded at draw time.
      figure(target).click();
      const original = block(target);
      const nodeDesc = (original as HTMLElement & { pmViewDesc?: { dirty: number } }).pmViewDesc;
      if (!nodeDesc) throw new Error('no view desc');
      nodeDesc.dirty = 3;
      // Trigger a ProseMirror update without changing the diagram source.
      target.view.dispatch(target.state.tr.insertText(' ', 1));
      expect(block(target)).not.toBe(original);
      await vi.waitFor(() => {
        expect(drawDiagram).toHaveBeenCalledTimes(2);
      });
    } finally {
      document.documentElement.classList.remove('dark');
    }
  });

  it('keeps the last picture when an open diagram view is rebuilt before an invalid edit', async () => {
    const target = create(BODY);
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('drawn');
    });
    figure(target).click();
    const original = block(target);
    // Simulate ProseMirror marking the node view NODE_DIRTY (for example after native DOM mutation
    // or a remote block replacement) so the node view is destroyed and reconstructed while open.
    const nodeDesc = (original as HTMLElement & { pmViewDesc?: { dirty: number } }).pmViewDesc;
    if (!nodeDesc) throw new Error('no view desc');
    nodeDesc.dirty = 3;
    type(target, '\n');
    expect(block(target)).not.toBe(original);
    expect(block(target).getAttribute('data-state')).toBe('drawn');
    expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe(FLOW);
    type(target, '  bad -->');
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('error');
    });
    expect(figure(target).hasAttribute('data-stale')).toBe(true);
    expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe(FLOW);
  });

  it('makes a new line of Shift+Enter too, and stays open while a selection is drawn out of it', () => {
    const target = create(BODY);
    figure(target).click();
    press(target, { key: 'Enter', shiftKey: true });
    expect(stored(target)).toEqual(
      doc(paragraph(text('before')), code('mermaid', `${FLOW}\n`), paragraph(text('after'))),
    );
    const { from } = target.state.selection;
    target.view.dispatch(
      target.state.tr.setSelection(
        TextSelection.create(target.state.doc, from, target.state.doc.content.size - 1),
      ),
    );
    expect(isOpen(target)).toBe(true);
  });

  it('does not say the same error again when the next drawing fails the same way', async () => {
    const target = create(doc(code('mermaid', 'flowchart LR\n  bad -->')));
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('error');
    });
    figure(target).click();
    const problem = block(target).querySelector('.doc-diagram-problem');
    const said = problem?.firstChild;
    expect(said?.textContent).toBe('This diagram cannot be drawn.');
    type(target, ' ');
    await vi.waitFor(() => {
      expect(drawDiagram).toHaveBeenCalledTimes(2);
    });
    // After that drawing has been shown: the words in the live region are the same ones.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(problem?.firstChild).toBe(said);
  });

  it('shows no source while the first picture is being drawn', async () => {
    drawDiagram.mockImplementationOnce(() => new Promise<Drawn>(() => undefined));
    const target = create(BODY);
    expect(block(target).getAttribute('data-state')).toBe('pending');
    expect(figure(target).textContent).toBe('');
    expect(figure(target).getAttribute('aria-busy')).toBe('true');
    // The drawing that never ends is the one asked for here, not by a later test.
    await vi.waitFor(() => {
      expect(drawDiagram).toHaveBeenCalledTimes(1);
    });
  });

  it('draws again once the drawing under way is done, when the page turned dark meanwhile', async () => {
    let finish: (value: Drawn) => void = () => undefined;
    drawDiagram.mockImplementationOnce(
      () =>
        new Promise<Drawn>((resolve) => {
          finish = resolve;
        }),
    );
    create(BODY);
    await vi.waitFor(() => {
      expect(drawDiagram).toHaveBeenCalledTimes(1);
    });
    document.documentElement.classList.add('dark');
    try {
      await new Promise((resolve) => setTimeout(resolve, 20));
      finish({ ok: true, picture: picture(FLOW) });
      await vi.waitFor(() => {
        expect(drawDiagram).toHaveBeenCalledTimes(2);
      });
    } finally {
      document.documentElement.classList.remove('dark');
    }
  });

  it('draws again a moment after the typing stops, once for a burst', async () => {
    vi.useFakeTimers();
    try {
      const target = create(doc(code('mermaid', 'pie\n  "a" : 1')));
      await vi.advanceTimersByTimeAsync(10);
      expect(drawDiagram).toHaveBeenCalledTimes(1);
      figure(target).click();
      type(target, '\n  "b" : 2');
      await vi.advanceTimersByTimeAsync(100);
      expect(drawDiagram).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1000);
      expect(drawDiagram).toHaveBeenCalledTimes(2);
      expect(drawDiagram).toHaveBeenLastCalledWith('pie\n  "a" : 1\n  "b" : 2');
      expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe(
        'pie\n  "a" : 1\n  "b" : 2',
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it('never puts an old picture over a newer one', async () => {
    const target = create(doc(code('mermaid', 'pie')));
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('drawn');
    });
    // The first drawing is slow; the source changes twice meanwhile.
    let finish: (value: Drawn) => void = () => undefined;
    drawDiagram.mockImplementationOnce(
      () =>
        new Promise<Drawn>((resolve) => {
          finish = resolve;
        }),
    );
    figure(target).click();
    type(target, ' 1');
    await vi.waitFor(() => {
      expect(drawDiagram).toHaveBeenLastCalledWith('pie 1');
    });
    type(target, '2');
    finish({ ok: true, picture: picture('pie 1') });
    await vi.waitFor(() => {
      expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe('pie 12');
    });
  });

  it('keeps the last picture, dimmed, while the source cannot be drawn, and says why under it', async () => {
    const target = create(BODY);
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('drawn');
    });
    figure(target).click();
    type(target, ' bad');
    await vi.waitFor(() => {
      expect(block(target).getAttribute('data-state')).toBe('error');
    });
    expect(figure(target).hasAttribute('data-stale')).toBe(true);
    expect(figure(target).querySelector('svg')?.getAttribute('data-source')).toBe(FLOW);
    const problem = block(target).querySelector('.doc-diagram-problem');
    expect(problem?.getAttribute('aria-live')).toBe('polite');
    expect(problem?.textContent).toContain('This diagram cannot be drawn.');
    // Typing on does not say it again (the same words, not new ones).
    const said = problem?.firstChild;
    type(target, ' more');
    expect(problem?.firstChild).toBe(said);
    // Closed, the source shows in place of the picture.
    press(target, { key: 'Escape' });
    expect(figure(target).querySelector('svg')).toBeNull();
    expect(figure(target).querySelector('.doc-diagram-copy')?.textContent).toBe(`${FLOW} bad more`);
  });

  it('puts what was typed in the undo of the body', () => {
    const target = create(BODY);
    figure(target).click();
    type(target, '\n  B --> C');
    press(target, { key: 'Escape' });
    expect(stored(target)).toEqual(
      doc(
        paragraph(text('before')),
        code('mermaid', `${FLOW}\n  B --> C`),
        paragraph(text('after')),
      ),
    );
    target.commands.undo();
    expect(stored(target)).toEqual(BODY);
    // The undo does not open the place again.
    expect(isOpen(target)).toBe(false);
  });
});
