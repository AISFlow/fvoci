/**
 * How a reference is drawn: three states that never look alike, a title
 * only for what the reader may see, and a link only where there is a screen.
 */
import { describe, expect, it, vi } from 'vitest';

import {
  drawReference,
  drawReferences,
  openClickedReference,
  type ReferenceDrawing,
  type ShownReference,
} from '$lib/editor/reference-chip.ts';

const SEEN = '01930000-0000-7000-8000-00000000b001';
const PAGE = '01930000-0000-7000-8000-00000000b002';
const ASKED = '01930000-0000-7000-8000-00000000b003';
const UNSEEN = '01930000-0000-7000-8000-00000000b004';

const SHOWN: Record<string, ShownReference | null> = {
  [SEEN]: { title: 'Plan', icon: '🚀', kind: 'card', href: '/w/ws/boards/b/v/c' },
  [PAGE]: { title: '', icon: '', kind: 'page' },
  [UNSEEN]: null,
};

function chip(id: string): HTMLElement {
  const element = document.createElement('span');
  element.setAttribute('data-reference', '');
  element.setAttribute('data-block-id', id);
  return element;
}

const drawing = (open?: (id: string) => void): ReferenceDrawing => ({
  shown: (id) => SHOWN[id],
  ...(open ? { open } : {}),
});

describe('a reference as it is drawn', () => {
  it('shows the icon and the title as a link to what it names', () => {
    const element = chip(SEEN);
    drawReference(element, drawing());
    expect(element.dataset.state).toBe('shown');
    expect(element.dataset.kind).toBe('card');
    const link = element.querySelector('a');
    expect(link?.getAttribute('href')).toBe('/w/ws/boards/b/v/c');
    expect(link?.textContent).toBe('🚀Plan');
    // The icon is not read out as a word of the title.
    expect(link?.querySelector('.reference-icon')?.getAttribute('aria-hidden')).toBe('true');
  });

  it('shows what has no screen with its title and no link, and a title for what has none', () => {
    const element = chip(PAGE);
    drawReference(element, drawing());
    expect(element.dataset.state).toBe('shown');
    expect(element.querySelector('a')).toBeNull();
    expect(element.textContent).toBe('Untitled');
  });

  it('says of what the reader may not see only that, and the same for an identifier that was emptied', () => {
    const shown = vi.fn(drawing().shown);
    for (const id of [UNSEEN, '', 'not-an-identifier']) {
      const element = chip(id);
      drawReference(element, { shown });
      expect([id, element.dataset.state, element.textContent]).toEqual([
        id,
        'unseen',
        "Link you can't see",
      ]);
      expect(element.querySelector('a')).toBeNull();
    }
    // What is no identifier is not asked for.
    expect(shown.mock.calls).toEqual([[UNSEEN]]);
    // Nothing to draw from: nothing is claimed of anything.
    const bare = chip(SEEN);
    drawReference(bare, undefined);
    expect(bare.dataset.state).toBe('unseen');
  });

  it('is pending while it is asked, which is not what the reader may not see', () => {
    const element = chip(ASKED);
    drawReference(element, drawing());
    expect(element.dataset.state).toBe('pending');
    expect(element.getAttribute('aria-busy')).toBe('true');
    expect(element.getAttribute('role')).toBe('img');
    expect(element.getAttribute('aria-label')).toBe('Loading link');
    expect(element.textContent).not.toContain('see');
  });

  it('draws anew when what it shows changes, and leaves the element alone when nothing does', () => {
    const root = document.createElement('div');
    root.append(chip(SEEN), chip(UNSEEN));
    const now: Record<string, ShownReference | null | undefined> = { ...SHOWN, [SEEN]: undefined };
    const from: ReferenceDrawing = { shown: (id) => now[id] };
    drawReferences(root, from);
    const [first] = root.querySelectorAll<HTMLElement>('span[data-reference]');
    expect(first?.dataset.state).toBe('pending');

    now[SEEN] = { title: 'Plan', icon: '', kind: 'card', href: '/x' };
    drawReferences(root, from);
    expect(first?.dataset.state).toBe('shown');
    expect(first?.hasAttribute('aria-busy')).toBe(false);
    const link = first?.querySelector('a');
    drawReferences(root, from);
    expect(first?.querySelector('a')).toBe(link);

    now[SEEN] = { title: 'Renamed', icon: '', kind: 'card', href: '/x' };
    drawReferences(root, from);
    expect(first?.textContent).toBe('Renamed');
  });

  it('opens what a plain click on the link names inside the app, and leaves any other click alone', () => {
    const open = vi.fn();
    const root = document.createElement('div');
    root.append(chip(SEEN), chip(UNSEEN));
    drawReferences(root, drawing(open));
    root.addEventListener('click', (event) => {
      openClickedReference(event, drawing(open));
    });
    const click = (target: Element, init: MouseEventInit = {}): boolean =>
      target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, ...init }));

    const title = root.querySelector('a .reference-title');
    if (!title) throw new Error('no link');
    // Not prevented: the browser would go on with it.
    expect(click(title, { ctrlKey: true })).toBe(true);
    expect(click(title, { button: 1 })).toBe(true);
    expect(open).not.toHaveBeenCalled();
    expect(click(title)).toBe(false);
    expect(open).toHaveBeenCalledExactlyOnceWith(SEEN);
    const unseen = root.querySelector('[data-state="unseen"]');
    if (!unseen) throw new Error('no unseen reference');
    expect(click(unseen)).toBe(true);
    expect(open).toHaveBeenCalledOnce();
  });
});
