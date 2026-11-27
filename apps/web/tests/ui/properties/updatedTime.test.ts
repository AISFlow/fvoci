/** The updated time of a card: read from the card, shown on the reader's clock, never editable. */
import { describe, expect, it } from 'vitest';

import { at, BERLIN, mount, NEW_YORK, screen, SEOUL, testCard } from './support.ts';

// The card of the tests was last changed at 18:30 on 6 March 2026 in Seoul.

describe('an updated time property', () => {
  it('shows when the card was last changed, on the clock of the account', () => {
    const { container } = mount('updatedTime', undefined, { regional: SEOUL });
    expect(container).toHaveTextContent('March 06 at 6:30 PM');
  });

  it('shows the same moment on the clock of a reader whose account is elsewhere', () => {
    const { container } = mount('updatedTime', undefined, { regional: NEW_YORK });
    expect(container).toHaveTextContent('March 06 at 4:30 AM');
  });

  it('writes the hours the way the account says', () => {
    const { container } = mount('updatedTime', undefined, { regional: BERLIN });
    expect(container).toHaveTextContent('March 06 at 10:30');
  });

  it('follows the card when it changes again', async () => {
    const mounted = mount('updatedTime', undefined);
    const card = testCard();
    card.updatedAt = at('Asia/Seoul', 2026, 3, 9, 8, 15);
    await mounted.setProps({ card });
    expect(mounted.container).toHaveTextContent('March 09 at 8:15 AM');
  });

  it('ignores a value stored under the property', () => {
    const { container } = mount('updatedTime', 'something stored');
    expect(container).toHaveTextContent('March 06 at 6:30 PM');
  });

  it('cannot be edited in any mode', () => {
    const { onChange } = mount('updatedTime', undefined, { mode: 'edit', startOpen: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('looks the same read only and in display mode', () => {
    const readOnly = mount('updatedTime', undefined, { readOnly: true });
    expect(readOnly.container).toHaveTextContent('March 06 at 6:30 PM');
    readOnly.unmount();
    const display = mount('updatedTime', undefined, { mode: 'display' });
    expect(display.container).toHaveTextContent('March 06 at 6:30 PM');
  });
});
