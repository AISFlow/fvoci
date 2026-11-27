/**
 * Component and screen-reader wording tests for Gantt relation editing
 * (`RelationEditor.svelte`) and 4-kind × lag accessible labels.
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  arrowAccessibleLabel,
  barLateSentences,
  relationBadge,
} from '$lib/components/properties/relation-labels.ts';
import RelationEditor from '$lib/features/views/gantt/RelationEditor.svelte';
import type { ArrowLine } from '$lib/features/views/gantt/chart.svelte.ts';
import { setLocale } from '$lib/i18n/paraglide/runtime.js';
import { createCard, relationLink, setCardProperty, type Card } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

afterEach(async () => {
  cleanup();
  await setLocale('en', { reload: false });
});

function makeArrow(overrides: Partial<ArrowLine> = {}): ArrowLine {
  return {
    key: 'card-a\u0000card-b',
    from: 'card-a',
    to: 'card-b',
    kind: 'fs',
    lagDays: 0,
    late: false,
    loop: false,
    path: 'M0 0.5L1 0.5',
    head: 'M1 0.5L0.6 0.36L0.6 0.64Z',
    hitPath: 'M0.5 0.5L0.5 1.5',
    labelX: 1,
    labelY: 1,
    badge: '',
    label: 'Design to Build, Finish to start.',
    fromTitle: 'Design',
    toTitle: 'Build',
    ...overrides,
  };
}

function makeBoard(targetCard: Card, apply = true) {
  const setCardPropertyMock = vi
    .fn()
    .mockImplementation((cardId: string, propertyId: string, value: unknown) => {
      if (apply && cardId === targetCard.id) {
        setCardProperty(targetCard, propertyId, value as Parameters<typeof setCardProperty>[2]);
      }
      return Promise.resolve();
    });
  const board = {
    cards: [targetCard],
    setCardProperty: setCardPropertyMock,
  } as unknown as BoardStore;
  return { board, setCardPropertyMock };
}

describe('RelationEditor', () => {
  it('edits kind and lagDays and preserves sibling and hidden links on save', async () => {
    const target = createCard();
    target.id = 'card-b';
    setCardProperty(target, 'after', [
      relationLink('card-hidden', 'sf', -4),
      relationLink('card-a', 'fs', 0),
      relationLink('card-other', 'ff', 2),
    ]);
    const { board, setCardPropertyMock } = makeBoard(target);
    const onclosed = vi.fn();

    render(RelationEditor, {
      props: {
        arrow: makeArrow(),
        open: true,
        anchor: null,
        board,
        dependencyPropertyId: 'after',
        sheet: true,
        onclosed,
      },
    });

    await fireEvent.click(screen.getByRole('radio', { name: /SS/ }));
    await fireEvent.click(screen.getByTestId('relation-lag-inc'));
    await fireEvent.click(screen.getByTestId('relation-lag-inc'));
    await fireEvent.click(screen.getByTestId('relation-save'));

    expect(setCardPropertyMock).toHaveBeenCalledExactlyOnceWith('card-b', 'after', [
      relationLink('card-hidden', 'sf', -4),
      relationLink('card-a', 'ss', 2),
      relationLink('card-other', 'ff', 2),
    ]);
    expect(onclosed).toHaveBeenCalledWith(false, 'card-b', 'card-a');
  });

  it('rejects out-of-range lagDays and disables Save until valid', async () => {
    const target = createCard();
    target.id = 'card-b';
    setCardProperty(target, 'after', [relationLink('card-a', 'fs', 0)]);
    const { board, setCardPropertyMock } = makeBoard(target);

    render(RelationEditor, {
      props: {
        arrow: makeArrow(),
        open: true,
        anchor: null,
        board,
        dependencyPropertyId: 'after',
        sheet: true,
        onclosed: vi.fn(),
      },
    });

    const input = screen.getByTestId('relation-lag-input');
    await fireEvent.input(input, { target: { value: '4000' } });
    expect(screen.getByRole('alert')).toHaveTextContent('-3650');
    expect(screen.getByTestId('relation-save')).toBeDisabled();
    expect(setCardPropertyMock).not.toHaveBeenCalled();

    await fireEvent.input(input, { target: { value: '-3' } });
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByTestId('relation-save')).toBeEnabled();
    await fireEvent.click(screen.getByTestId('relation-save'));
    expect(setCardPropertyMock).toHaveBeenCalledExactlyOnceWith('card-b', 'after', [
      relationLink('card-a', 'fs', -3),
    ]);
  });

  it('removes only the edited link while preserving sibling and hidden links', async () => {
    const target = createCard();
    target.id = 'card-b';
    setCardProperty(target, 'after', [
      relationLink('card-hidden', 'sf', 5),
      relationLink('card-a', 'ss', 1),
      relationLink('card-other', 'ff', -1),
    ]);
    const { board, setCardPropertyMock } = makeBoard(target);
    const onclosed = vi.fn();

    render(RelationEditor, {
      props: {
        arrow: makeArrow({ kind: 'ss', lagDays: 1 }),
        open: true,
        anchor: null,
        board,
        dependencyPropertyId: 'after',
        sheet: true,
        onclosed,
      },
    });

    await fireEvent.click(screen.getByTestId('relation-remove'));
    expect(setCardPropertyMock).toHaveBeenCalledExactlyOnceWith('card-b', 'after', [
      relationLink('card-hidden', 'sf', 5),
      relationLink('card-other', 'ff', -1),
    ]);
    expect(onclosed).toHaveBeenCalledWith(true, 'card-b', 'card-a');
  });

  it('supports WAI-ARIA arrow keys in the relation kind radiogroup and suppresses announcements on reverted writes', async () => {
    const target = createCard();
    target.id = 'card-b';
    setCardProperty(target, 'after', [relationLink('card-a', 'fs', 0)]);
    const { board, setCardPropertyMock } = makeBoard(target, false);

    render(RelationEditor, {
      props: {
        arrow: makeArrow(),
        open: true,
        anchor: null,
        board,
        dependencyPropertyId: 'after',
        sheet: true,
        onclosed: vi.fn(),
      },
    });

    const fsRadio = screen.getByRole('radio', { name: /FS/ });
    expect(fsRadio).toHaveAttribute('tabindex', '0');
    await fireEvent.keyDown(fsRadio, { key: 'ArrowRight' });
    expect(screen.getByRole('radio', { name: /SS/ })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: /SS/ })).toHaveAttribute('tabindex', '0');
    expect(fsRadio).toHaveAttribute('tabindex', '-1');

    await fireEvent.click(screen.getByTestId('relation-save'));
    expect(setCardPropertyMock).toHaveBeenCalledTimes(1);
    // Write was rolled back (`apply = false`), so screen reader status stays silent.
    expect(screen.getByTestId('gantt-relation-status')).toHaveTextContent(/^$/);
  });
});

describe('relation screen-reader labels (en & ko)', () => {
  const titles: Record<string, string> = { a: 'Design', b: 'Build' };
  const titleOf = (id: string): string | undefined => titles[id];
  const list = (items: string[]): string => items.join(' and ');

  it('formats badges, arrow labels, and 4-kind × lag late sentences in English', async () => {
    await setLocale('en', { reload: false });
    expect(relationBadge('fs', 0)).toBe('');
    expect(relationBadge('fs', 2)).toBe('FS +2d');
    expect(relationBadge('ss', 0)).toBe('SS');
    expect(relationBadge('ff', -1)).toBe('FF -1d');

    expect(arrowAccessibleLabel('Design', 'Build', 'ss', 2, true, false)).toBe(
      'Design to Build, Start to start, +2 days lag. Out of order.',
    );
    expect(arrowAccessibleLabel('Design', 'Build', 'ff', -1, false, true)).toBe(
      'Design to Build, Finish to finish, -1 day lead. On a loop.',
    );

    expect(
      barLateSentences(
        [
          { from: 'a', to: 'c', kind: 'fs', lagDays: 0 },
          { from: 'b', to: 'c', kind: 'fs', lagDays: 0 },
        ],
        titleOf,
        list,
      ),
    ).toEqual(['Out of order: starts before the end of Design and Build.']);

    expect(
      barLateSentences(
        [
          { from: 'a', to: 'c', kind: 'ss', lagDays: 2 },
          { from: 'b', to: 'c', kind: 'ff', lagDays: -1 },
          { from: 'a', to: 'c', kind: 'sf', lagDays: 0 },
        ],
        titleOf,
        list,
      ),
    ).toEqual([
      'Out of order: starts fewer than 2 days after the start of Design.',
      'Out of order: finishes more than 1 day before the end of Build.',
      'Out of order: finishes before the start of Design.',
    ]);
  });

  it('formats badges, arrow labels, and 4-kind × lag late sentences in Korean', async () => {
    await setLocale('ko', { reload: false });
    expect(relationBadge('ss', 2)).toBe('SS +2일');
    expect(relationBadge('ff', -1)).toBe('FF -1일');
    expect(arrowAccessibleLabel('설계', '구현', 'ss', 2, true, false)).toBe(
      '설계에서 구현(으)로, 시작→시작, +2일 간격. 어긋남.',
    );
    expect(
      barLateSentences([{ from: 'a', to: 'c', kind: 'sf', lagDays: 3 }], titleOf, list),
    ).toEqual(['어긋남: Design 시작 후 3일 간격보다 먼저 끝납니다.']);
  });
});
