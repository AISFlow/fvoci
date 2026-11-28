/**
 * The other end of a relation in a card's detail: which cards name the card
 * (components/properties/kinds/referrers.ts) and the list of them under the
 * property (ReverseRelations.svelte), alone and in the card's properties.
 */
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { referrersOf } from '$lib/components/properties/kinds/referrers.ts';
import ReverseRelations from '$lib/components/properties/kinds/ReverseRelations.svelte';
import CardProperties from '$lib/features/card/CardProperties.svelte';
import { relationLink } from '$lib/logic/relations.ts';
import { createCard, type Card, type PropertyValue } from '$lib/logic/wire.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';

import { openFirstBoard, setup } from '../../state/support.ts';

const cardOf = (
  id: string,
  title: string,
  properties: Record<string, PropertyValue> = {},
): Card => {
  const card = createCard();
  return { ...card, id, title, fields: { ...card.fields, properties } };
};

afterEach(() => {
  cleanup();
});

describe('the cards that name a card', () => {
  const target = cardOf('t', 'Target');
  const plain = cardOf('a', 'A', { blocks: [relationLink('t')] });
  const linked = cardOf('b', 'B', { blocks: [{ card: 't', kind: 'fs', lagDays: 2 }] });
  const other = cardOf('c', 'C', { blocks: [relationLink('a')], parentOf: [relationLink('t')] });
  const self = cardOf('t2', 'Self', { blocks: [relationLink('t2')] });

  it('are those whose value of the property names it, in the order of the board', () => {
    const cards = [other, linked, target, plain, self];
    expect(referrersOf(cards, 'blocks', 't').map((card) => card.id)).toEqual(['b', 'a']);
  });

  it('are read per property', () => {
    expect(referrersOf([other, target], 'parentOf', 't').map((card) => card.id)).toEqual(['c']);
    expect(referrersOf([other, target], 'blocks', 't')).toEqual([]);
  });

  it('do not count a legacy list of identifiers, which names no card', () => {
    expect(referrersOf([cardOf('d', 'D', { blocks: ['t'] }), target], 'blocks', 't')).toEqual([]);
  });

  it('leave out a card that names itself and a template', () => {
    expect(referrersOf([self], 'blocks', 't2')).toEqual([]);
    const template = { ...plain, fields: { ...plain.fields, isTemplate: true } };
    expect(referrersOf([template, target], 'blocks', 't')).toEqual([]);
  });
});

describe('the list under the property', () => {
  it('draws nothing where no card names this one', () => {
    const { container } = render(ReverseRelations, { props: { referrers: [] } });
    expect(container.querySelector('[data-slot="relation-referrers"]')).toBeNull();
  });

  it('names each card, and a press opens it', async () => {
    const onOpenCard = vi.fn<(cardId: string) => void>();
    render(ReverseRelations, {
      props: { referrers: [cardOf('a', 'Alpha'), cardOf('b', '')], onOpenCard },
    });
    const list = screen.getByRole('list', { name: 'Cards that point to this card' });
    expect(
      within(list)
        .getAllByRole('button')
        .map((button) => button.textContent.trim()),
    ).toEqual(['Alpha', 'Untitled']);
    await fireEvent.click(within(list).getByRole('button', { name: 'Open Alpha' }));
    expect(onOpenCard).toHaveBeenCalledExactlyOnceWith('a');
  });
});

const SEED: SeedTemplate = {
  key: 'reverse-relations',
  onboarding: false,
  board: {
    id: 'b',
    title: 'Work',
    description: '',
    icon: '',
    showDescription: false,
    cardProperties: [
      { id: 'blocks', name: 'Blocks', type: 'relation', options: [] },
      { id: 'note', name: 'Note', type: 'text', options: [] },
    ],
  },
  views: [],
  cards: [
    {
      id: 'c1',
      title: 'Design',
      fields: { icon: '', isTemplate: false, properties: {} },
      body: null,
    },
    {
      id: 'c2',
      title: 'Build',
      fields: { icon: '', isTemplate: false, properties: { blocks: [relationLink('c1')] } },
      body: null,
    },
  ],
  files: [],
};

describe("a card's properties", () => {
  it('list under a relation the cards whose value of it names the card', async () => {
    const { board } = await openFirstBoard(setup([SEED]));
    const design = board.cards.find((card) => card.title === 'Design');
    const build = board.cards.find((card) => card.title === 'Build');
    if (!design || !build) throw new Error('the cards are missing');
    const onOpenCard = vi.fn<(cardId: string) => void>();
    render(CardProperties, { props: { board, view: undefined, card: design, onOpenCard } });
    await tick();
    const row = screen
      .getByTestId('card-properties')
      .querySelector<HTMLElement>('[data-property-type="relation"]');
    if (!row) throw new Error('no relation row');
    const list = within(row).getByRole('list', { name: 'Cards that point to this card' });
    await fireEvent.click(within(list).getByRole('button', { name: 'Open Build' }));
    expect(onOpenCard).toHaveBeenCalledExactlyOnceWith(build.id);
    // The other end has nothing to show: Build names Design, nothing names Build.
    expect(
      screen.getByTestId('card-properties').querySelectorAll('[data-slot="relation-referrers"]'),
    ).toHaveLength(1);
  });
});
