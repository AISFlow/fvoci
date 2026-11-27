/** Who created a card: read from the card, never editable. */
import { describe, expect, it } from 'vitest';

import { mount, screen, testCard } from './support.ts';

// The card of the tests was created by user-1 (jisoo, no display name).

describe('a created by property', () => {
  it('shows who created the card', () => {
    const { container } = mount('createdBy', undefined);
    expect(container).toHaveTextContent('jisoo');
  });

  it('shows someone who is not among the people as an unknown user', () => {
    const card = testCard();
    card.createdBy = 'user-gone';
    const { container } = mount('createdBy', undefined, { card });
    expect(container).toHaveTextContent('Unknown user');
  });

  it('names an author who left from the people known, though they are not offered', () => {
    const card = testCard();
    card.createdBy = 'user-gone';
    const named = [{ id: 'user-gone', username: 'leaver', displayName: 'Lee' }];
    const { container } = mount('createdBy', undefined, { card, named });
    expect(container).toHaveTextContent('Lee (@leaver)');
    expect(container).not.toHaveTextContent('Unknown user');
  });

  it('shows an author whose account was deleted as a deleted user', () => {
    const card = testCard();
    card.createdBy = 'user-gone';
    // What is left of the account: its identifier, and no name.
    const named = [{ id: 'user-gone', username: '', displayName: '' }];
    const { container } = mount('createdBy', undefined, { card, named });
    expect(container).toHaveTextContent('Deleted user');
    expect(container).not.toHaveTextContent('Unknown user');
  });

  it('shows nothing for a card without a creator', () => {
    const card = testCard();
    card.createdBy = '';
    const { container } = mount('createdBy', undefined, { card });
    expect(container).toHaveTextContent('');
  });

  it('ignores a value stored under the property: the card itself is the source', () => {
    const { container } = mount('createdBy', 'user-3');
    expect(container).toHaveTextContent('jisoo');
    expect(container).not.toHaveTextContent('ada');
  });

  it('cannot be edited in any mode', () => {
    const { onChange } = mount('createdBy', undefined, { mode: 'edit', startOpen: true });
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('looks the same read only and in display mode', () => {
    const readOnly = mount('createdBy', undefined, { readOnly: true });
    expect(readOnly.container).toHaveTextContent('jisoo');
    readOnly.unmount();
    const display = mount('createdBy', undefined, { mode: 'display' });
    expect(display.container).toHaveTextContent('jisoo');
  });
});
