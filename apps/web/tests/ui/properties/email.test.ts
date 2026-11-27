/** The value of an email property: one address, checked for its shape before it is saved. */
import { describe, expect, it } from 'vitest';

import { leave, mount, press, screen, type } from './support.ts';

const field = (): HTMLInputElement => screen.getByRole('textbox', { name: 'My email' });

describe('an email property', () => {
  it('shows its value, and a placeholder without one', () => {
    mount('email', 'ada@example.org');
    expect(field()).toHaveValue('ada@example.org');
    expect(field()).toHaveAttribute('placeholder', 'Empty');
  });

  it('saves an address on Enter, once', async () => {
    const { onChange } = mount('email', '');
    await type(field(), 'ada@example.org');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('ada@example.org');
  });

  it('saves an address on leaving the field', async () => {
    const { onChange } = mount('email', 'old@example.org');
    await type(field(), 'new@example.org');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('new@example.org');
  });

  it('does not save what is not an address', async () => {
    const { onChange } = mount('email', '');
    await type(field(), 'not an address');
    await press(field(), 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveFocus();
    expect(field()).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('Enter an email address');
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveValue('not an address');
  });

  it('puts the stored address back on Escape', async () => {
    const { onChange } = mount('email', 'ada@example.org');
    await type(field(), 'ada@');
    await press(field(), 'Escape');
    expect(field()).toHaveValue('ada@example.org');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('can be emptied, starting from an address', async () => {
    const { onChange } = mount('email', 'ada@example.org');
    await type(field(), '');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
  });

  it('can be set starting from nothing', async () => {
    // What is checked is the typed value, not the stored one: an empty email
    // property can be given a value.
    const { onChange } = mount('email', '');
    await type(field(), 'first@example.org');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('first@example.org');
  });

  it('asks a phone for the email keys', () => {
    mount('email', '');
    expect(field()).toHaveAttribute('inputmode', 'email');
  });
});

describe('an email property that may not be edited', () => {
  it('is plain text, not a link', () => {
    mount('email', 'ada@example.org', { readOnly: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByText('ada@example.org')).toBeInTheDocument();
  });

  it('shows the placeholder without a value', () => {
    mount('email', '', { mode: 'display' });
    expect(screen.getByText('Empty')).toBeInTheDocument();
  });
});

describe('an email property that starts in the editing state', () => {
  it('has the focus at once', () => {
    mount('email', '', { startOpen: true });
    expect(field()).toHaveFocus();
  });
});
