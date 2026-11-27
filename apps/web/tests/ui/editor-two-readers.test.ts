/**
 * Places held across what somebody else writes, between two readers of the
 * backend in memory: each has the editor of the same body, bound to the
 * document of their own room's client, and what one writes reaches the
 * other through the room (editor-shared-places.test.ts holds the same
 * places against changes put into one document by hand).
 *
 * - An image going up lands where it was put though the other wrote a
 *   paragraph above it, and a word into its line, before it was up.
 * - The source written into a formula's box goes into that formula though
 *   the other wrote above it and beside it meanwhile.
 * - The place of a block formula stays on its formula.
 */
import type { Editor } from '@tiptap/core';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { openPlace, placeOf } from '$lib/editor/editing-place.ts';
import { setFileSource } from '$lib/editor/files.ts';
import type { DocNode } from '$lib/editor/schema.ts';
import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';

import { setup } from '../state/support.ts';
import { doc, math, mathBlock, paragraph, text } from '../support/body-docs.ts';

vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

setFileSource({ url: (fileId) => `/files/${fileId}` });

const hears = (): BodyDocHears => ({ state: () => undefined, discarded: () => undefined });

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** The blocks of an editor's document, a word each. */
const blocks = (editor: Editor): string[] => {
  const found: string[] = [];
  editor.state.doc.forEach((node) => {
    found.push(node.type.name === 'paragraph' ? node.textContent : node.type.name);
  });
  return found;
};

/** What a formula of an editor's document holds, in the order they come. */
const formulas = (editor: Editor): string[] => {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === 'inlineMath' || node.type.name === 'blockMath') {
      found.push(String(node.attrs.latex));
    }
  });
  return found;
};

interface Two {
  mine: Editor;
  theirs: Editor;
  /** Lets an image of the board go up: the identifier a document may name. */
  fileId: string;
  rooms: BodyDoc[];
}

const opened: BodyDoc[] = [];

/** Two readers who write, each with the editor of a card whose body is `body`. */
async function two(body: DocNode, upload?: (file: File) => Promise<string>): Promise<Two> {
  const { world, backend, other } = setup();
  const card = [...world.blocks.values()].find((block) => block.type === 'card' && block.body);
  if (!card) throw new Error('the seed has no card with a body');
  card.body = body;
  const fileId = 'file-of-the-board';
  world.files.set(fileId, {
    fileId,
    containerId: card.containerId,
    name: 'a.png',
    size: 3,
    contentType: 'image/png',
  });
  const rooms = [backend, other].map((reader) =>
    reader.openDoc('w', card.containerId, card.id, hears()),
  );
  opened.push(...rooms);
  await vi.waitFor(() => {
    for (const room of rooms) expect(room.state.status).toBe('joined');
  });
  const [mine, theirs] = rooms.map((room, index) => {
    const { container } = render(BodyEditor, {
      doc: null,
      collab: room.state.doc,
      canWrite: true,
      ...(index === 0 && upload ? { uploadFile: upload } : {}),
    });
    return editorIn(container);
  }) as [Editor, Editor];
  await vi.waitFor(() => {
    expect(blocks(theirs)).toEqual(blocks(mine));
    expect(mine.state.doc.childCount).toBeGreaterThan(0);
  });
  return { mine, theirs, fileId, rooms };
}

/** Pastes an image file the way a browser reports it. */
function pasteImage(editor: Editor): void {
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: () => string };
  };
  event.clipboardData = {
    files: [new File(['png'], 'a.png', { type: 'image/png' })],
    getData: () => '',
  };
  editor.view.someProp('handlePaste', (handle) =>
    handle(editor.view, event as unknown as ClipboardEvent, editor.state.doc.slice(0, 0)),
  );
}

afterEach(() => {
  for (const room of opened.splice(0)) room.close();
  document.querySelectorAll('.doc-math-editing').forEach((node) => {
    node.remove();
  });
});

describe('two readers of one body, and the places each holds', () => {
  it('an image lands where it was put though the other wrote above it and into its line before it was up', async () => {
    let finish = (): void => undefined;
    let fileId = '';
    const upload = (): Promise<string> =>
      new Promise((resolve) => {
        finish = () => {
          resolve(fileId);
        };
      });
    const at = await two(doc(paragraph(text('first')), paragraph(text('second'))), upload);
    fileId = at.fileId;
    // The caret at the end of "second", and an image pasted there: it goes up slowly.
    at.mine.commands.setTextSelection(at.mine.state.doc.content.size - 1);
    pasteImage(at.mine);

    // Meanwhile the other writes a paragraph above, and a word into that line.
    at.theirs.commands.insertContentAt(0, paragraph(text('above')));
    const second = at.theirs.state.doc.content.size - 'second'.length - 1;
    at.theirs.commands.insertContentAt(second, 'the ');
    await vi.waitFor(() => {
      expect(blocks(at.mine)).toEqual(['above', 'first', 'the second']);
    });

    finish();
    await vi.waitFor(() => {
      expect(blocks(at.mine)).toEqual(['above', 'first', 'the second', 'image']);
      expect(blocks(at.theirs)).toEqual(['above', 'first', 'the second', 'image']);
    });
    await vi.waitFor(() => {
      for (const room of at.rooms) expect(room.state.pending).toBe(false);
    });
    expect(at.rooms.map((room) => room.state.status)).toEqual(['joined', 'joined']);
  });

  it("the source written into a formula's box goes into that formula though the other wrote above and beside it", async () => {
    const at = await two(
      doc(paragraph(text('a '), math('x'), text(' b ')), paragraph(text('c '), math('z'))),
    );
    // The reader opens the box of the first formula.
    at.mine.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea');
    if (!field) throw new Error('no box');

    // Meanwhile the other writes a paragraph above, and right before the formula.
    at.theirs.commands.insertContentAt(0, paragraph(text('new')));
    at.theirs.commands.insertContentAt('new'.length + 2 + 'a '.length + 1, 'and ');
    await vi.waitFor(() => {
      expect(blocks(at.mine)).toEqual(['new', 'a and  b ', 'c ']);
    });

    field.value = 'y';
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(formulas(at.mine)).toEqual(['y', 'z']);
    await vi.waitFor(() => {
      expect(formulas(at.theirs)).toEqual(['y', 'z']);
      expect(blocks(at.theirs)).toEqual(blocks(at.mine));
    });
  });

  it('the place of a block formula stays on its formula while the other writes above it, and closes when they delete it', async () => {
    const at = await two(doc(paragraph(text('first')), mathBlock('x^2'), mathBlock('y^2')));
    // The place of the first formula: it is the second block.
    openPlace(at.mine.view, at.mine.state.doc.child(0).nodeSize);
    const open = (): string | undefined => {
      const place = placeOf(at.mine.state);
      return place ? String(at.mine.state.doc.nodeAt(place.pos)?.attrs.latex) : undefined;
    };
    expect(open()).toBe('x^2');

    at.theirs.commands.insertContentAt(0, [paragraph(text('one')), paragraph(text('two'))]);
    await vi.waitFor(() => {
      expect(blocks(at.mine)).toEqual(['one', 'two', 'first', 'blockMath', 'blockMath']);
    });
    expect(open()).toBe('x^2');

    // The other deletes that formula: the place closes, and is not the next formula's.
    let first = -1;
    at.theirs.state.doc.forEach((node, offset) => {
      if (first < 0 && node.type.name === 'blockMath') first = offset;
    });
    at.theirs.commands.deleteRange({ from: first, to: first + 1 });
    await vi.waitFor(() => {
      expect(formulas(at.mine)).toEqual(['y^2']);
    });
    expect(placeOf(at.mine.state)).toBeNull();
  });
});
