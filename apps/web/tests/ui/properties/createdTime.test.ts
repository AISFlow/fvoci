/** The created time of a card: read from the card, shown on the reader's clock, never editable. */
import { describe, expect, it } from 'vitest';

import { BERLIN, mount, NEW_YORK, screen, SEOUL } from './support.ts';

// The card of the tests was created at 09:00 on 5 March 2026 in Seoul.

describe('a created time property', () => {
  it('shows when the card was created, on the clock of the account', () => {
    const { container } = mount('createdTime', undefined, { regional: SEOUL });
    expect(container).toHaveTextContent('March 05 at 9:00 AM');
  });

  it('shows the same moment as another day and time to a reader whose account is elsewhere', () => {
    const { container } = mount('createdTime', undefined, { regional: NEW_YORK });
    expect(container).toHaveTextContent('March 04 at 7:00 PM');
  });

  it('writes the hours the way the account says', () => {
    const { container } = mount('createdTime', undefined, { regional: BERLIN });
    expect(container).toHaveTextContent('March 05 at 01:00');
  });

  it('is written in the language of the reader', () => {
    const { container } = mount('createdTime', undefined, { locale: 'ko' });
    expect(container).toHaveTextContent('3월 05일 오전 9:00');
  });

  it('shows the year when it is not the reader’s current year', () => {
    const { container } = mount('createdTime', undefined, {
      now: Date.UTC(2027, 5, 1),
    });
    expect(container).toHaveTextContent('March 05, 2026 at 9:00 AM');
  });

  it('ignores a value stored under the property: the card itself is the source', () => {
    const { container } = mount('createdTime', 'something stored');
    expect(container).toHaveTextContent('March 05 at 9:00 AM');
    expect(container).not.toHaveTextContent('something stored');
  });

  it('cannot be edited in any mode', () => {
    const { onChange } = mount('createdTime', undefined, { mode: 'edit', startOpen: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('looks the same read only and in display mode', () => {
    const readOnly = mount('createdTime', undefined, { readOnly: true });
    expect(readOnly.container).toHaveTextContent('March 05 at 9:00 AM');
    readOnly.unmount();
    const display = mount('createdTime', undefined, { mode: 'display' });
    expect(display.container).toHaveTextContent('March 05 at 9:00 AM');
  });

  it('has no placeholder: a card always has a created time', () => {
    const { container } = mount('createdTime', undefined);
    expect(container).not.toHaveTextContent('Empty');
  });
});
