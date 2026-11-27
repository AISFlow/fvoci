/** The value of a number property: text that has to start with a number, stored as text. */
import { describe, expect, it } from 'vitest';

import { leave, mount, press, screen, type } from './support.ts';

const field = (): HTMLInputElement => screen.getByRole('textbox', { name: 'My number' });

describe('a number property', () => {
  it('shows its value, and a placeholder without one', () => {
    mount('number', '42');
    expect(field()).toHaveValue('42');
    expect(field()).toHaveAttribute('placeholder', 'Empty');
  });

  it('saves a number as the text that was typed', async () => {
    const { onChange } = mount('number', '');
    await type(field(), '123');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('123');
  });

  it('saves on Enter, once', async () => {
    const { onChange } = mount('number', '1');
    await type(field(), '-12.5');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('-12.5');
  });

  it('does not save what is not a number: Enter keeps the person in the field', async () => {
    const { onChange } = mount('number', '1');
    await type(field(), 'abc');
    await press(field(), 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveFocus();
    expect(field()).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a number');
  });

  it('does not save what is not a number on leaving, and keeps it there marked as wrong', async () => {
    const { onChange } = mount('number', '1');
    await type(field(), 'abc');
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveValue('abc');
    expect(field()).toHaveAttribute('aria-invalid', 'true');
  });

  it('puts the stored number back on Escape', async () => {
    const { onChange } = mount('number', '1');
    await type(field(), 'abc');
    await press(field(), 'Escape');
    expect(field()).toHaveValue('1');
    expect(field()).not.toHaveAttribute('aria-invalid');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is no longer marked as wrong once it is a number again', async () => {
    const { onChange } = mount('number', '');
    await type(field(), 'x');
    expect(field()).toHaveAttribute('aria-invalid', 'true');
    await type(field(), '7');
    expect(field()).not.toHaveAttribute('aria-invalid');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('7');
  });

  it('takes a number followed by other text', async () => {
    const { onChange } = mount('number', '');
    await type(field(), '12 kg');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('12 kg');
  });

  it('can be emptied', async () => {
    const { onChange } = mount('number', '5');
    await type(field(), '');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
  });

  it('does not save an unchanged number', async () => {
    const { onChange } = mount('number', '5');
    await type(field(), '5');
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
  });

  it('asks a phone for the number keys and does not check spelling', () => {
    mount('number', '');
    expect(field()).toHaveAttribute('inputmode', 'decimal');
    expect(field()).toHaveAttribute('spellcheck', 'false');
  });

  it('does not keep what was typed when removed with something that is not a number', async () => {
    const mounted = mount('number', '5');
    await type(field(), 'abc');
    mounted.unmount();
    expect(mounted.onChange).not.toHaveBeenCalled();
  });
});

describe('a number property that may not be edited', () => {
  it('is plain text', () => {
    mount('number', '42', { readOnly: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByText('42')).toBeInTheDocument();
  });

  it('is plain text in display mode, with nothing where there is no value', () => {
    const { container } = mount('number', '', { mode: 'display', showEmptyPlaceholder: false });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(container).toHaveTextContent('');
  });
});

describe('a number property that starts in the editing state', () => {
  it('has the focus at once', () => {
    mount('number', '42', { startOpen: true });
    expect(field()).toHaveFocus();
  });
});
