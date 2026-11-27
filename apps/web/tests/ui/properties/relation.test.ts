/**
 * The value of a card relation property: other cards of the board, picked by
 * title, shown as chips that open the card. A card that is not there (deleted)
 * is not shown, and stays in the next value written.
 */
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  createCard,
  RELATION_LIMIT,
  relationLink,
  setCardProperty,
  type Card,
  type RelationLink,
} from '$lib/logic/wire.ts';

import { fireEvent, mount, press, screen, type, valueButton, within } from './support.ts';

const links = (...ids: string[]): RelationLink[] => ids.map((id) => relationLink(id));

afterEach(() => {
  vi.unstubAllGlobals();
});

/**
 * Holds the animation frames back until the returned function lets them run.
 * (Waiting for a real frame is no use here: a picker of the simulated page
 * closes by itself once time passes.)
 */
function holdFrames(): () => Promise<void> {
  const waiting: FrameRequestCallback[] = [];
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) =>
    waiting.push(callback),
  );
  vi.stubGlobal('cancelAnimationFrame', () => undefined);
  return async () => {
    for (const callback of waiting.splice(0)) callback(0);
    await tick();
  };
}

function cardsOf(titles: Record<string, string>): Map<string, Card> {
  return new Map(
    Object.entries(titles).map(([id, title]) => {
      const card = createCard();
      card.id = id;
      card.title = title;
      return [id, card];
    }),
  );
}

/** The board: the card under test (`card-1`) and three others. */
const CARDS = cardsOf({
  'card-1': 'A card',
  'card-2': 'Design the form',
  'card-3': 'Build the form',
  'card-4': '',
});

const button = (): HTMLElement => valueButton('My relation');
const search = (): HTMLInputElement => screen.getByRole('combobox', { name: 'My relation' });
const list = (): HTMLElement => screen.getByRole('listbox', { name: 'My relation' });
const rows = (): string[] =>
  within(list())
    .queryAllByRole('option')
    .map((row) => row.textContent.trim());
const chips = (container: HTMLElement): string[] =>
  [
    ...container.querySelectorAll('[data-slot="property-relation"] [data-slot="relation-chip"]'),
  ].map((chip) => chip.textContent.trim());

describe('a relation property, closed', () => {
  it('shows the cards it names by title, an untitled one as such', () => {
    const { container } = mount('relation', links('card-3', 'card-4'), { cards: CARDS });
    expect(chips(container)).toEqual(['Build the form', 'Untitled']);
  });

  it('does not show a card that is not there, nor one named twice', () => {
    const { container } = mount('relation', links('card-gone', 'card-2', 'card-2'), {
      cards: CARDS,
    });
    expect(chips(container)).toEqual(['Design the form']);
  });

  it('shows a placeholder when it names only cards that are not there', () => {
    const { container } = mount('relation', links('card-gone'), { cards: CARDS });
    expect(chips(container)).toEqual([]);
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
  });

  it('is empty for a value that is not a list of relation links', () => {
    const { container } = mount('relation', 'card-2', { cards: CARDS });
    expect(chips(container)).toEqual([]);
  });

  it('opens a card by its chip, named for a screen reader', async () => {
    const onOpenCard = vi.fn();
    mount('relation', links('card-2'), { cards: CARDS, onOpenCard });
    await fireEvent.click(screen.getByRole('button', { name: 'Open Design the form' }));
    expect(onOpenCard).toHaveBeenCalledExactlyOnceWith('card-2');
  });

  it('read only: chips without a picker', () => {
    const { container } = mount('relation', links('card-2'), { cards: CARDS, readOnly: true });
    expect(chips(container)).toEqual(['Design the form']);
    expect(screen.queryByRole('button', { name: /^My relation/ })).toBeNull();
  });
});

describe('a relation property, opened', () => {
  it('lists the other cards of the board by title, not the card itself', async () => {
    mount('relation', [], { cards: CARDS });
    await fireEvent.click(button());
    expect(rows()).toEqual(['Build the form', 'Design the form', 'Untitled']);
  });

  it('lists the picked cards last, marked, and Enter on the empty field adds a card', async () => {
    const { onChange } = mount('relation', links('card-2'), { cards: CARDS });
    await fireEvent.click(button());
    expect(rows()).toEqual(['Build the form', 'Untitled', 'Design the form']);
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(links('card-2', 'card-3'));
  });

  it('finds an untitled card by that word', async () => {
    mount('relation', links('card-2'), { cards: CARDS });
    await fireEvent.click(button());
    await type(search(), 'untit');
    expect(rows()).toEqual(['Untitled']);
    await type(search(), '');
    expect(within(list()).getByRole('option', { name: 'Design the form' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  it('narrows the list by title as the reader types, and picks with the keyboard', async () => {
    const { onChange } = mount('relation', [], { cards: CARDS });
    await fireEvent.click(button());
    await type(search(), 'build');
    expect(rows()).toEqual(['Build the form']);
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(links('card-3'));
  });

  it('adds a card, and takes out one that is pressed again', async () => {
    const { onChange } = mount('relation', links('card-2'), { cards: CARDS });
    await fireEvent.click(button());
    await fireEvent.click(within(list()).getByRole('option', { name: 'Build the form' }));
    expect(onChange).toHaveBeenLastCalledWith(links('card-2', 'card-3'));
    await fireEvent.click(within(list()).getByRole('option', { name: 'Design the form' }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it('preserves existing kind and lagDays on untouched and hidden links when adding or removing', async () => {
    const existing = [relationLink('card-gone', 'sf', 4), relationLink('card-2', 'ss', -3)];
    const { onChange } = mount('relation', existing, { cards: CARDS });
    await fireEvent.click(button());
    await fireEvent.click(within(list()).getByRole('option', { name: 'Build the form' }));
    expect(onChange).toHaveBeenLastCalledWith([
      relationLink('card-gone', 'sf', 4),
      relationLink('card-2', 'ss', -3),
      relationLink('card-3', 'fs', 0),
    ]);
  });

  it('shows non-default kind/lag badges on chips and edits them inline without touching hidden links', async () => {
    const existing = [relationLink('card-gone', 'sf', 4), relationLink('card-2', 'ss', 2)];
    const { container, onChange } = mount('relation', existing, {
      cards: CARDS,
      onOpenCard: vi.fn(),
    });
    expect(container.querySelector('[data-slot="relation-badge"]')).toHaveTextContent('SS +2d');
    expect(
      screen.getByRole('button', { name: 'Open Design the form, SS +2d' }),
    ).toBeInTheDocument();

    await fireEvent.click(button());
    const kindSelect = screen.getByLabelText('Relation type for Design the form');
    await fireEvent.change(kindSelect, { target: { value: 'ff' } });
    expect(onChange).toHaveBeenLastCalledWith([
      relationLink('card-gone', 'sf', 4),
      relationLink('card-2', 'ff', 2),
    ]);

    const lagInput = screen.getByLabelText('Lag days for Design the form');
    await fireEvent.input(lagInput, { target: { value: '-1' } });
    await fireEvent.change(lagInput, { target: { value: '-1' } });
    expect(onChange).toHaveBeenLastCalledWith([
      relationLink('card-gone', 'sf', 4),
      relationLink('card-2', 'ss', -1),
    ]);

    // Pressing Escape reverts dirty input instead of committing it on blur.
    onChange.mockClear();
    await fireEvent.input(lagInput, { target: { value: '99' } });
    await fireEvent.keyDown(lagInput, { key: 'Escape' });
    await fireEvent.blur(lagInput);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes out a card with the button next to its title, and Backspace takes the last', async () => {
    const { onChange } = mount('relation', links('card-2', 'card-3'), { cards: CARDS });
    await fireEvent.click(button());
    await fireEvent.click(screen.getByRole('button', { name: 'Remove Design the form' }));
    expect(onChange).toHaveBeenLastCalledWith(links('card-3'));
    await press(search(), 'Backspace');
    expect(onChange).toHaveBeenLastCalledWith(links('card-2'));
  });

  it('keeps the cards that are not there in the value it writes, where they were', async () => {
    const { onChange } = mount('relation', links('card-gone', 'card-2'), { cards: CARDS });
    await fireEvent.click(button());
    await fireEvent.click(within(list()).getByRole('option', { name: 'Build the form' }));
    expect(onChange).toHaveBeenLastCalledWith(links('card-gone', 'card-2', 'card-3'));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove Design the form' }));
    expect(onChange).toHaveBeenLastCalledWith(links('card-gone'));
  });

  it('offers nothing more when deleted cards fill the value, and leaves none out by itself', async () => {
    const many = cardsOf({ 'new-card': 'Another card' });
    const gone = Array.from({ length: RELATION_LIMIT }, (_, at) =>
      relationLink(`gone-${String(at)}`),
    );
    const frames = holdFrames();
    const { onChange } = mount('relation', gone, { cards: many });
    await fireEvent.click(button());
    expect(rows()).toEqual([]);
    await press(search(), 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    // Said in the live region, with a button that takes them out.
    await frames();
    expect(document.querySelector('[data-slot="relation-held-back"]')).toHaveTextContent(
      `Deleted cards hold ${String(RELATION_LIMIT)} of the places. Take them out to pick more.`,
    );
    await fireEvent.click(
      screen.getByRole('button', {
        name: `Take out the deleted cards: ${String(RELATION_LIMIT)}`,
      }),
    );
    expect(onChange).toHaveBeenCalledExactlyOnceWith([]);
  });

  it('takes out only the deleted cards with that button, and keeps the picked ones', async () => {
    const gone = Array.from({ length: RELATION_LIMIT - 1 }, (_, at) =>
      relationLink(`gone-${String(at)}`),
    );
    const { onChange } = mount('relation', [relationLink('card-2'), ...gone], { cards: CARDS });
    await fireEvent.click(button());
    await fireEvent.click(screen.getByRole('button', { name: /^Take out the deleted cards/ }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(links('card-2'));
  });

  it('offers no such button when taking the deleted cards out leaves no room', async () => {
    const many = cardsOf(
      Object.fromEntries(
        Array.from({ length: RELATION_LIMIT }, (_, at) => [`c${String(at)}`, `Card ${String(at)}`]),
      ),
    );
    mount(
      'relation',
      [...many.keys(), 'card-gone'].map((id) => relationLink(id)),
      { cards: many },
    );
    await fireEvent.click(button());
    expect(screen.queryByRole('button', { name: /^Take out the deleted cards/ })).toBeNull();
  });

  it('shows no such button while deleted cards leave room', async () => {
    mount('relation', links('card-gone', 'card-2'), { cards: CARDS });
    await fireEvent.click(button());
    expect(screen.queryByRole('button', { name: /^Take out the deleted cards/ })).toBeNull();
  });

  it(`picks no more than ${String(RELATION_LIMIT)} cards, and says so`, async () => {
    const many = cardsOf(
      Object.fromEntries(
        Array.from({ length: RELATION_LIMIT + 2 }, (_, at) => [
          `c${String(at)}`,
          `Card ${String(at)}`,
        ]),
      ),
    );
    const picked = [...many.keys()].slice(0, RELATION_LIMIT).map((id) => relationLink(id));
    const frames = holdFrames();
    const { onChange } = mount('relation', picked, { cards: many });
    await fireEvent.click(button());
    // Said once the live region is on the page, so that it is heard.
    expect(screen.getByRole('status')).toHaveTextContent(/^$/);
    await frames();
    expect(screen.getByRole('status')).toHaveTextContent(
      `No more than ${String(RELATION_LIMIT)} cards can be picked.`,
    );
    // The cards that are not picked are not offered.
    await type(search(), `Card ${String(RELATION_LIMIT + 1)}`);
    expect(rows()).toEqual([]);
    await press(search(), 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    // Taking one out still works.
    await type(search(), 'Card 0');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(picked.slice(1));
  });

  it('draws a limited number of rows on a board of thousands of cards, and says how many more', async () => {
    const many = cardsOf(
      Object.fromEntries(
        Array.from({ length: 3000 }, (_, at) => [`c${String(at)}`, `Task ${String(at)}`]),
      ),
    );
    mount('relation', [], { cards: many });
    await fireEvent.click(button());
    expect(rows()).toHaveLength(50);
    expect(screen.getByText('More cards: 2950. Type to narrow the list.')).toBeInTheDocument();
    await type(search(), 'Task 2999');
    expect(rows()).toEqual(['Task 2999']);
  });

  it('keeps every card of a value that names more than the limit when one is taken out', async () => {
    const many = cardsOf(
      Object.fromEntries(
        Array.from({ length: RELATION_LIMIT + 10 }, (_, at) => [
          `c${String(at)}`,
          `Card ${String(at)}`,
        ]),
      ),
    );
    const named = [...many.keys()].map((id) => relationLink(id));
    const { onChange } = mount('relation', named, { cards: many });
    await fireEvent.click(button());
    await fireEvent.click(screen.getByRole('button', { name: 'Remove Card 0' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(named.slice(1));
  });

  it('does not show the card itself when its value names it, and leaves it out when written', async () => {
    const { container, onChange } = mount('relation', links('card-1', 'card-2'), { cards: CARDS });
    expect(chips(container)).toEqual(['Design the form']);
    await fireEvent.click(button());
    await fireEvent.click(within(list()).getByRole('option', { name: 'Build the form' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(links('card-2', 'card-3'));
  });
});

describe('a relation read as an order', () => {
  const heldBack = (): HTMLElement =>
    document.querySelector<HTMLElement>('[data-slot="relation-held-back"]') ??
    (() => {
      throw new Error('no line about held back cards');
    })();

  /** card-2 comes after the card under test, and card-3 after card-2. */
  function ordered(): Map<string, Card> {
    const cards = cardsOf({
      'card-1': 'A card',
      'card-2': 'Design the form',
      'card-3': 'Build the form',
      'card-4': 'Ship it',
    });
    const named = (id: string): Card => {
      const found = cards.get(id);
      if (!found) throw new Error(`no ${id}`);
      return found;
    };
    setCardProperty(named('card-2'), 'property-relation', links('card-1'));
    setCardProperty(named('card-3'), 'property-relation', links('card-2'));
    return cards;
  }

  it('does not offer a card that would close a loop, however far, and says how many', async () => {
    const frames = holdFrames();
    mount('relation', [], { cards: ordered(), preventLoops: true });
    await fireEvent.click(button());
    expect(rows()).toEqual(['Ship it']);
    // The region is there empty first, and says it a frame later: that is what is heard.
    expect(heldBack()).toHaveTextContent(/^$/);
    await frames();
    expect(heldBack()).toHaveTextContent('Not offered: 2. Picking them would make a loop.');
    // Counted among what was typed: the card looked for is there, and held back.
    await type(search(), 'design');
    expect(rows()).toEqual([]);
    expect(heldBack()).toHaveTextContent('Not offered: 1. Picking them would make a loop.');
    await type(search(), 'ship');
    expect(rows()).toEqual(['Ship it']);
    expect(heldBack()).toHaveTextContent(/^$/);
  });

  it('says only that the value is full when it is', async () => {
    const cards = ordered();
    for (let at = 0; at < RELATION_LIMIT; at += 1) {
      const extra = createCard();
      extra.id = `extra-${String(at)}`;
      extra.title = `Extra ${String(at)}`;
      cards.set(extra.id, extra);
    }
    const picked = [...cards.keys()]
      .filter((id) => id.startsWith('extra-'))
      .map((id) => relationLink(id));
    const frames = holdFrames();
    mount('relation', picked, { cards, preventLoops: true });
    await fireEvent.click(button());
    await frames();
    expect(heldBack()).toHaveTextContent(
      `No more than ${String(RELATION_LIMIT)} cards can be picked.`,
    );
  });

  it('offers every card where the relation is not read as an order', async () => {
    mount('relation', [], { cards: ordered() });
    await fireEvent.click(button());
    expect(rows()).toEqual(['Build the form', 'Design the form', 'Ship it']);
  });

  it('marks a card on a loop that is there already, and lets it be taken out', async () => {
    const { container, onChange } = mount('relation', links('card-3', 'card-4'), {
      cards: ordered(),
      preventLoops: true,
      onOpenCard: vi.fn(),
    });
    expect(chips(container)).toEqual(['Build the form', 'Ship it']);
    expect(screen.getByRole('button', { name: 'Open Build the form, in a loop' })).toHaveAttribute(
      'data-loop',
    );
    expect(screen.getByRole('button', { name: 'Open Ship it' })).not.toHaveAttribute('data-loop');
    await fireEvent.click(button());
    expect(rows()).toEqual(['Build the form', 'Ship it']);
    await fireEvent.click(within(list()).getByRole('option', { name: 'Build the form' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(links('card-4'));
  });
});
