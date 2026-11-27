/**
 * The carets of the others in the editor of a body edited together
 * (`editor/carets.ts` in BodyEditor.svelte): a line where each is, with
 * their name and colour looked up by the user the room named, their name
 * shown for a while after they moved, and nothing of it for a screen reader.
 */
import type { Editor } from '@tiptap/core';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { NAME_SHOWN } from '$lib/editor/carets.ts';
import { caretState, caretTold } from '$lib/state/memory/caret-frame.ts';
import { Carets } from '$lib/state/doc-carets.ts';

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** A shared document of two lines, and its carets as the room's client holds them. */
function shared(): { doc: Y.Doc; held: Carets; told: Uint8Array[]; line: Y.XmlText } {
  const doc = new Y.Doc();
  const line = new Y.XmlText('Hello caret');
  const first = new Y.XmlElement('paragraph');
  const second = new Y.XmlElement('paragraph');
  doc.getXmlFragment(BODY_FIELD).insert(0, [first, second]);
  first.insert(0, [line]);
  second.insert(0, [new Y.XmlText('Second line')]);
  const told: Uint8Array[] = [];
  const held = new Carets(doc, (update) => {
    told.push(update);
    return true;
  });
  return { doc, held, told, line };
}

/** What the room tells of a caret of `userId` from `from` to `to` in the first line. */
function tell(
  at: { held: Carets; line: Y.XmlText },
  caret: { number: number; clock: number; userId: string },
  from: number,
  to = from,
): void {
  const place = (index: number): Record<string, unknown> =>
    JSON.parse(JSON.stringify(Y.createRelativePositionFromTypeIndex(at.line, index))) as Record<
      string,
      unknown
    >;
  const state = caretState(caret.userId, { anchor: place(from), head: place(to) });
  at.held.take(caretTold(caret.number, caret.clock, state));
}

const NAMES: Record<string, string> = { ada: 'Ada Lovelace', kim: '김하늘' };

function mount(at: { doc: Y.Doc; held: Carets }): HTMLElement {
  const { container } = render(BodyEditor, {
    doc: null,
    collab: at.doc,
    carets: { awareness: at.held.awareness, colorOf: (userId: string) => `var(--of-${userId})` },
    resolveUser: (userId: string) => NAMES[userId],
  });
  return container;
}

const caretsIn = (container: HTMLElement): HTMLElement[] => [
  ...container.querySelectorAll<HTMLElement>('.doc-caret'),
];

afterEach(() => {
  // What the binding queued runs before the timers are the machine's again: a timer of its
  // own that is thrown away would leave it waiting for good, in every test after.
  if (vi.isFakeTimers()) vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

describe("the others' carets in the editor", () => {
  it('draws a line where each is, named and coloured by the user the room named, and none for a screen reader', async () => {
    const at = shared();
    const container = mount(at);
    expect(caretsIn(container)).toEqual([]);
    tell(at, { number: 41, clock: 1, userId: 'ada' }, 10);
    tell(at, { number: 42, clock: 1, userId: 'kim' }, 2, 8);
    await vi.waitFor(() => {
      expect(caretsIn(container)).toHaveLength(2);
    });
    const [ada, kim] = caretsIn(container).sort((a, b) =>
      (a.dataset.user ?? '').localeCompare(b.dataset.user ?? ''),
    ) as [HTMLElement, HTMLElement];
    expect(ada.querySelector<HTMLElement>('.doc-caret-name')?.dataset.name).toBe('Ada Lovelace');
    expect(ada.style.getPropertyValue('--caret')).toBe('var(--of-ada)');
    expect(ada.getAttribute('aria-hidden')).toBe('true');
    expect(kim.querySelector<HTMLElement>('.doc-caret-name')?.dataset.name).toBe('김하늘');
    // The name is drawn, not written: the text in the editor is the body's alone.
    expect(editorIn(container).view.dom.textContent).toBe('Hello caretSecond line');
    // The other's selection is tinted in their colour.
    const selected = container.querySelector<HTMLElement>('.doc-caret-selection');
    expect(selected?.textContent).toBe('llo ca');
    expect(selected?.style.getPropertyValue('--caret')).toBe('var(--of-kim)');
    // The text of the body is what it was: a caret is no part of it.
    expect(editorIn(container).state.doc.firstChild?.textContent).toBe('Hello caret');
    at.held.destroy();
  });

  it('shows a name for a while after its caret moved, and the line stays', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const at = shared();
    const container = mount(at);
    tell(at, { number: 41, clock: 1, userId: 'ada' }, 5);
    await vi.waitFor(() => {
      expect(caretsIn(container)).toHaveLength(1);
    });
    expect(caretsIn(container)[0]?.hasAttribute('data-named')).toBe(true);
    // (Waiting for the caret moved the timers a little.)
    vi.advanceTimersByTime(NAME_SHOWN - 1000);
    expect(caretsIn(container)[0]?.hasAttribute('data-named')).toBe(true);
    vi.advanceTimersByTime(1000);
    expect(caretsIn(container)[0]?.hasAttribute('data-named')).toBe(false);
    // Moved again, the name is back, on the line where the caret is now.
    tell(at, { number: 41, clock: 2, userId: 'ada' }, 9);
    await vi.waitFor(() => {
      expect(caretsIn(container)[0]?.hasAttribute('data-named')).toBe(true);
    });
    expect(caretsIn(container)).toHaveLength(1);
    // Said again where it is (every fifteen seconds), the name stays away.
    vi.advanceTimersByTime(NAME_SHOWN);
    tell(at, { number: 41, clock: 3, userId: 'ada' }, 9);
    expect(caretsIn(container)[0]?.hasAttribute('data-named')).toBe(false);
    at.held.destroy();
  });

  it('takes a caret away when the room says it is gone, and leaves no timer when the editor goes', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const at = shared();
    const { container, unmount } = render(BodyEditor, {
      doc: null,
      collab: at.doc,
      carets: { awareness: at.held.awareness, colorOf: () => 'red' },
    });
    tell(at, { number: 41, clock: 1, userId: 'ada' }, 5);
    tell(at, { number: 42, clock: 1, userId: 'kim' }, 3);
    await vi.waitFor(() => {
      expect(caretsIn(container)).toHaveLength(2);
    });
    // A name nobody resolves is the identifier the room gave.
    const names = caretsIn(container).map(
      (caret) => caret.querySelector<HTMLElement>('.doc-caret-name')?.dataset.name,
    );
    expect(names).toEqual(expect.arrayContaining(['ada', 'kim']));
    at.held.take(caretTold(41, 2, null));
    await vi.waitFor(() => {
      expect(caretsIn(container)).toHaveLength(1);
    });
    unmount();
    at.held.destroy();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("tells the room where the reader's own caret is while the editor has the focus, and draws none for it", async () => {
    const at = shared();
    const container = mount(at);
    // The view's own focus, which does not scroll (no layout here).
    editorIn(container).view.focus();
    await vi.waitFor(() => {
      expect(at.told.length).toBeGreaterThan(0);
    });
    expect(at.held.awareness.getLocalState()?.cursor).toBeTruthy();
    expect(caretsIn(container)).toEqual([]);
    // Leaving the editor withdraws it.
    editorIn(container).view.dom.blur();
    await vi.waitFor(() => {
      expect(at.held.awareness.getLocalState()?.cursor).toBeNull();
    });
    at.held.destroy();
  });

  it('draws no caret whose place is in something deleted, or not there yet, and goes on', async () => {
    const at = shared();
    const container = mount(at);
    tell(at, { number: 41, clock: 1, userId: 'ada' }, 5);
    await vi.waitFor(() => {
      expect(caretsIn(container)).toHaveLength(1);
    });
    // The line the caret is in is deleted by somebody: the caret stays told, with no place.
    at.doc.getXmlFragment(BODY_FIELD).delete(0, 1);
    await vi.waitFor(() => {
      expect(editorIn(container).state.doc.firstChild?.textContent).toBe('Second line');
    });
    expect(caretsIn(container).every((caret) => caret.isConnected)).toBe(true);
    // A caret at an item this reader has not been sent yet is not drawn, and breaks nothing.
    const notYet = { type: null, tname: null, item: { client: 987_654, clock: 3 }, assoc: 0 };
    at.held.take(caretTold(42, 1, caretState('kim', { anchor: notYet, head: notYet })));
    tell(at, { number: 41, clock: 2, userId: 'ada' }, 0);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(caretsIn(container).some((caret) => caret.dataset.user === 'kim')).toBe(false);
    expect(editorIn(container).state.doc.textContent).toBe('Second line');
    at.held.destroy();
  });

  it('shows a name that became known after its caret was drawn', async () => {
    const at = shared();
    const names: Record<string, string> = {};
    const { container } = render(BodyEditor, {
      doc: null,
      collab: at.doc,
      carets: { awareness: at.held.awareness, colorOf: () => 'red' },
      resolveUser: (userId: string) => names[userId],
    });
    const name = (): string | undefined =>
      caretsIn(container)[0]?.querySelector<HTMLElement>('.doc-caret-name')?.dataset.name;
    tell(at, { number: 41, clock: 1, userId: 'ada' }, 5);
    await vi.waitFor(() => {
      expect(name()).toBe('ada');
    });
    // The names arrive: the caret says the name the next time it is looked at.
    names.ada = 'Ada Lovelace';
    tell(at, { number: 41, clock: 2, userId: 'ada' }, 6);
    await vi.waitFor(() => {
      expect(name()).toBe('Ada Lovelace');
    });
    at.held.destroy();
  });
});

describe('a syllable composed in the body', () => {
  /** The editor of a shared document, and what it said of composing. */
  function composing(): { said: boolean[]; text: HTMLElement; unmount: () => void } {
    const at = shared();
    const said: boolean[] = [];
    const { container, unmount } = render(BodyEditor, {
      doc: null,
      collab: at.doc,
      onComposing: (on: boolean) => said.push(on),
    });
    return { said, text: editorIn(container).view.dom, unmount };
  }
  const stroke = (on: Element, type: string): void => {
    on.dispatchEvent(new CompositionEvent(type, { bubbles: true }));
  };

  it('is said with each stroke, and its end once the editor has read what it left', async () => {
    const { said, text } = composing();
    stroke(text, 'compositionstart');
    stroke(text, 'compositionupdate');
    expect(said).toEqual([true, true]);
    stroke(text, 'compositionend');
    // Not within the event: the editor reads the page right after it.
    expect(said).toEqual([true, true]);
    await Promise.resolve();
    expect(said).toEqual([true, true, false]);
    // The end of a composition that never was one of the body says nothing.
    stroke(text, 'compositionend');
    await Promise.resolve();
    expect(said).toHaveLength(3);
  });

  it('is not one in a field drawn into the body, and ends with the editor that goes away in the middle of it', () => {
    const { said, text, unmount } = composing();
    const field = document.createElement('textarea');
    text.append(field);
    stroke(field, 'compositionstart');
    expect(said).toEqual([]);
    field.remove();
    stroke(text, 'compositionstart');
    unmount();
    expect(said).toEqual([true, false]);
  });
});
