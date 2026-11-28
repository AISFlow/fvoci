/**
 * The gallery view over a board store on the in-memory backend: the cards
 * of the view in its order, opening one, making one.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import Gallery from '$lib/features/views/gallery/Gallery.svelte';
import type { BoardView } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { openFirstBoard, STATUS } from '../../state/support.ts';

afterEach(() => {
  cleanup();
});

interface Shown {
  board: BoardStore;
  view: BoardView;
  onOpenCard: ReturnType<typeof vi.fn<(cardId: string) => void>>;
  container: HTMLElement;
}

async function show(fields: Partial<BoardView['fields']> = {}): Promise<Shown> {
  const { board } = await openFirstBoard();
  const first = board.orderedViews[0];
  if (!first) throw new Error('no view');
  const view = { ...first, fields: { ...first.fields, viewType: 'gallery' as const, ...fields } };
  const onOpenCard = vi.fn<(cardId: string) => void>();
  const { container } = render(Gallery, {
    board,
    view,
    cards: board.cardsOf(view),
    cardHref: (cardId: string) => `/board/b/v/${cardId}`,
    onOpenCard,
  });
  return { board, view, onOpenCard, container };
}

const titles = (container: HTMLElement): string[] =>
  [...container.querySelectorAll('[data-slot="card-title-text"]')].map((title) =>
    title.textContent.trim(),
  );

describe('the gallery', () => {
  it('shows the cards of the view in its order, as a list named for a screen reader', async () => {
    const { container } = await show();
    expect(titles(container)).toEqual(['Alpha', 'Beta', 'Gamma']);
    expect(screen.getByRole('list', { name: 'Cards' })).toBeInTheDocument();
    expect(screen.getAllByRole('link').map((card) => card.getAttribute('aria-labelledby'))).toEqual(
      [...container.querySelectorAll('[data-slot="card-title"]')].map((title) => title.id),
    );
  });

  it('draws the large face, with the properties the view turned on', async () => {
    const { container } = await show({ shownProperties: [STATUS] });
    const faces = [...container.querySelectorAll('[data-slot="card-face"]')];
    expect(faces).toHaveLength(3);
    expect(faces.every((face) => face.getAttribute('data-size') === 'large')).toBe(true);
    expect(faces[2]?.querySelector('[data-slot="option-chip"]')).toHaveTextContent('Doing');
    // The card list says which cards have a body; the gallery shows no more of it than that.
    expect(faces[0]?.querySelector('[data-slot="card-has-body"]')).not.toBeNull();
    expect(faces[1]?.querySelector('[data-slot="card-has-body"]')).toBeNull();
  });

  it('opens a card on a press, and on Enter', async () => {
    const { board, onOpenCard } = await show();
    const beta = board.realCards.find((card) => card.title === 'Beta');
    const [, second, third] = screen.getAllByRole('link');
    if (!second || !third || !beta) throw new Error('no cards');
    await fireEvent.click(second);
    expect(onOpenCard).toHaveBeenCalledExactlyOnceWith(beta.id);
    await fireEvent.keyDown(third, { key: 'Enter' });
    expect(onOpenCard).toHaveBeenCalledTimes(2);
    // Other keys are not a press.
    await fireEvent.keyDown(third, { key: ' ' });
    expect(onOpenCard).toHaveBeenCalledTimes(2);
  });

  it('gives every card a menu of its own, which a press on does not open the card', async () => {
    const { onOpenCard } = await show();
    const menus = screen.getAllByRole('button', { name: /^Card menu: / });
    expect(menus.map((menu) => menu.getAttribute('aria-label'))).toEqual([
      'Card menu: Alpha',
      'Card menu: Beta',
      'Card menu: Gamma',
    ]);
    const first = menus[0];
    if (!first) throw new Error('no menu');
    await fireEvent.click(first);
    expect(onOpenCard).not.toHaveBeenCalled();
  });

  it('makes a card with "+ New" and opens it', async () => {
    const { board, onOpenCard } = await show();
    await fireEvent.click(screen.getByRole('button', { name: '+ New' }));
    await waitFor(() => {
      expect(onOpenCard).toHaveBeenCalledOnce();
    });
    expect(board.realCards).toHaveLength(4);
    expect(board.card(onOpenCard.mock.calls[0]?.[0])).toBeDefined();
  });

  it('gives a new card what the filter of the view asks for', async () => {
    const { board, onOpenCard } = await show({
      filter: {
        operation: 'and',
        filters: [{ propertyId: STATUS, condition: 'includes', values: ['done'] }],
      },
    });
    await fireEvent.click(screen.getByRole('button', { name: '+ New' }));
    await waitFor(() => {
      expect(onOpenCard).toHaveBeenCalledOnce();
    });
    expect(board.card(onOpenCard.mock.calls[0]?.[0])?.fields.properties[STATUS]).toBe('done');
  });

  it('selects a card with Ctrl, and another view of the board starts with nothing selected', async () => {
    const { board } = await openFirstBoard();
    const first = board.orderedViews[0];
    if (!first) throw new Error('no view');
    const view = { ...first, fields: { ...first.fields, viewType: 'gallery' as const } };
    const props = {
      board,
      view,
      cards: board.cardsOf(view),
      cardHref: (cardId: string) => `/board/b/v/${cardId}`,
      onOpenCard: vi.fn<(cardId: string) => void>(),
    };
    const { rerender } = render(Gallery, props);
    const [card] = screen.getAllByRole('link');
    if (!card) throw new Error('no cards');
    await fireEvent.click(card, { ctrlKey: true });
    expect(screen.getByTestId('selection-bar')).toHaveTextContent('1 selected');
    expect(props.onOpenCard).not.toHaveBeenCalled();

    // ✕ lets it go, and the focus it had goes back to the card.
    const clear = screen.getByRole('button', { name: 'Clear selection' });
    clear.focus();
    await fireEvent.click(clear);
    await waitFor(() => {
      expect(screen.queryByTestId('selection-bar')).toBeNull();
    });
    await tick();
    expect(document.activeElement).toBe(card);
    await fireEvent.click(card, { ctrlKey: true });

    await rerender({ ...props, view: { ...view, id: `${view.id}-other` } });
    expect(screen.queryByTestId('selection-bar')).toBeNull();
  });
});
