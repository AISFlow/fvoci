/** Who last changed a card: read from the card, never editable. */
import { describe, expect, it } from 'vitest';

import { mount, screen, testCard } from './support.ts';

// The card of the tests was last changed by user-2 (minho, shown as Mino).

describe('an updated by property', () => {
  it('shows who last changed the card', () => {
    const { container } = mount('updatedBy', undefined);
    expect(container).toHaveTextContent('minho');
    expect(container).not.toHaveTextContent('jisoo');
  });

  it('shows the display name with the login name', () => {
    const { container } = mount('updatedBy', undefined);
    expect(container).toHaveTextContent('Mino (@minho)');
  });

  it('follows the card when someone else changes it', async () => {
    const mounted = mount('updatedBy', undefined);
    const card = testCard();
    card.modifiedBy = 'user-3';
    await mounted.setProps({ card });
    expect(mounted.container).toHaveTextContent('ada');
  });

  it('shows someone who is not among the people as an unknown user', () => {
    const card = testCard();
    card.modifiedBy = 'user-gone';
    const { container } = mount('updatedBy', undefined, { card });
    expect(container).toHaveTextContent('Unknown user');
  });

  it('cannot be edited in any mode', () => {
    const { onChange } = mount('updatedBy', undefined, { mode: 'edit', startOpen: true });
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('looks the same read only and in display mode', () => {
    const readOnly = mount('updatedBy', undefined, { readOnly: true });
    expect(readOnly.container).toHaveTextContent('minho');
    readOnly.unmount();
    const display = mount('updatedBy', undefined, { mode: 'display' });
    expect(display.container).toHaveTextContent('minho');
  });
});
