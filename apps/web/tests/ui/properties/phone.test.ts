/** The value of a phone property: one line, saved as typed; nothing is checked. */
import { describe, expect, it } from 'vitest';

import { leave, mount, press, screen, type } from './support.ts';

const field = (): HTMLInputElement => screen.getByRole('textbox', { name: 'My phone' });

describe('a phone property', () => {
  it('shows its value, and a placeholder without one', () => {
    mount('phone', '+82 10-1234-5678');
    expect(field()).toHaveValue('+82 10-1234-5678');
    expect(field()).toHaveAttribute('placeholder', 'Empty');
  });

  it('saves on Enter, once', async () => {
    const { onChange } = mount('phone', '');
    await type(field(), '010-1234-5678');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('010-1234-5678');
  });

  it('saves on leaving the field', async () => {
    const { onChange } = mount('phone', '1');
    await type(field(), '02) 123-4567');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('02) 123-4567');
  });

  it('takes any text: a number is not checked', async () => {
    const { onChange } = mount('phone', '');
    await type(field(), 'ask at the front desk');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('ask at the front desk');
    expect(field()).not.toHaveAttribute('aria-invalid');
  });

  it('puts the stored number back on Escape', async () => {
    const { onChange } = mount('phone', '010');
    await type(field(), '011');
    await press(field(), 'Escape');
    expect(field()).toHaveValue('010');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('asks a phone for the telephone keys', () => {
    mount('phone', '');
    expect(field()).toHaveAttribute('inputmode', 'tel');
  });
});

describe('a phone property that may not be edited', () => {
  it('is plain text, not a link', () => {
    mount('phone', '010-1234-5678', { readOnly: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByText('010-1234-5678')).toBeInTheDocument();
  });

  it('shows the placeholder without a value', () => {
    mount('phone', '', { mode: 'display' });
    expect(screen.getByText('Empty')).toBeInTheDocument();
  });
});

describe('a phone property that starts in the editing state', () => {
  it('has the focus at once', () => {
    mount('phone', '', { startOpen: true });
    expect(field()).toHaveFocus();
  });
});
