/** The value of a text property: one line, edited in place. */
import { describe, expect, it } from 'vitest';

import { leave, mount, press, screen, type } from './support.ts';

const field = (): HTMLInputElement => screen.getByRole('textbox', { name: 'My text' });

describe('a text property', () => {
  it('shows its value in a field named after the property', () => {
    mount('text', 'hello');
    expect(field()).toHaveValue('hello');
  });

  it('shows a placeholder where there is no value, when asked to', async () => {
    const mounted = mount('text', '');
    expect(field()).toHaveAttribute('placeholder', 'Empty');
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(field()).not.toHaveAttribute('placeholder', 'Empty');
  });

  it('saves what was typed on Enter, once', async () => {
    const { onChange } = mount('text', 'hello');
    await type(field(), 'hello world');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('hello world');
    // Enter also leaves the field, and leaving does not save a second time.
    expect(field()).not.toHaveFocus();
  });

  it('saves on leaving the field', async () => {
    const { onChange } = mount('text', '');
    await type(field(), '한글 입력');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('한글 입력');
  });

  it('puts the stored value back on Escape and saves nothing', async () => {
    const { onChange } = mount('text', 'hello');
    await type(field(), 'something else');
    await press(field(), 'Escape');
    expect(field()).toHaveValue('hello');
    expect(field()).not.toHaveFocus();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('does not save a value that was not changed', async () => {
    const { onChange } = mount('text', 'hello');
    await type(field(), 'hello');
    await press(field(), 'Enter');
    field().focus();
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
  });

  it('saves an emptied field as an empty value', async () => {
    const { onChange } = mount('text', 'hello');
    await type(field(), '');
    await press(field(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
  });

  it('leaves Enter and Escape to an input method that is composing', async () => {
    const { onChange } = mount('text', '');
    await type(field(), '한');
    await press(field(), 'Enter', { isComposing: true });
    await press(field(), 'Escape', { isComposing: true });
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveValue('한');
    expect(field()).toHaveFocus();
  });

  it('does not take Enter with a modifier key as a command', async () => {
    const { onChange } = mount('text', '');
    await type(field(), 'draft');
    await press(field(), 'Enter', { shiftKey: true });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('keeps Escape to itself, so that what the field is in does not close too', async () => {
    mount('text', 'hello');
    let reached = false;
    document.body.addEventListener('keydown', () => (reached = true), { once: true });
    await type(field(), 'changed');
    await press(field(), 'Escape');
    expect(reached).toBe(false);
  });

  it('follows a value that changes from outside', async () => {
    const mounted = mount('text', 'hello');
    await mounted.setValue('changed elsewhere');
    expect(field()).toHaveValue('changed elsewhere');
  });

  it('keeps what a person is typing when the value changes from outside', async () => {
    const mounted = mount('text', 'hello');
    await type(field(), 'my draft');
    await mounted.setValue('changed elsewhere');
    expect(field()).toHaveValue('my draft');
  });

  it('saves what was typed when it is removed while being edited', async () => {
    const mounted = mount('text', 'hello');
    await type(field(), 'unsaved');
    mounted.unmount();
    expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith('unsaved');
  });

  it('checks spelling, unlike the other one-line kinds', () => {
    mount('text', '');
    expect(field()).toHaveAttribute('spellcheck', 'true');
  });

  it('shows a list left behind by another kind as text, joined', () => {
    mount('text', ['a', 'b']);
    expect(field()).toHaveValue('a,b');
  });
});

describe('a text property that may not be edited', () => {
  it('is plain text when read only', async () => {
    const { onChange } = mount('text', 'hello', { readOnly: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByText('hello')).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is plain text in display mode', () => {
    mount('text', 'hello', { mode: 'display' });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByText('hello')).toBeInTheDocument();
  });

  it('shows the placeholder, or nothing, without a value', async () => {
    const mounted = mount('text', '', { readOnly: true });
    expect(screen.getByText('Empty')).toBeInTheDocument();
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(screen.queryByText('Empty')).toBeNull();
  });

  it('does not start editing even when asked to start open', () => {
    mount('text', 'hello', { readOnly: true, startOpen: true });
    expect(screen.queryByRole('textbox')).toBeNull();
  });
});

describe('a text property that starts in the editing state', () => {
  it('has the focus at once', () => {
    mount('text', 'hello', { startOpen: true });
    expect(field()).toHaveFocus();
  });
});
