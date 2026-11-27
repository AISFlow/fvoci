/** The value of a checkbox property: pressing it changes it; `true` or nothing is stored. */
import { describe, expect, it } from 'vitest';

import { fireEvent, mount, screen } from './support.ts';

const box = (): HTMLElement => screen.getByRole('checkbox', { name: 'My checkbox' });

describe('a checkbox property', () => {
  it('is checked when the stored value is true', () => {
    mount('checkbox', 'true');
    expect(box()).toBeChecked();
  });

  it('is unchecked without a value', () => {
    mount('checkbox', '');
    expect(box()).not.toBeChecked();
  });

  it('is unchecked for any stored value other than true', () => {
    mount('checkbox', 'false');
    expect(box()).not.toBeChecked();
  });

  it('stores true when an unchecked box is pressed', async () => {
    const { onChange } = mount('checkbox', '');
    await fireEvent.click(box());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('true');
  });

  it('stores an empty value when a checked box is pressed', async () => {
    const { onChange } = mount('checkbox', 'true');
    await fireEvent.click(box());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
  });

  it('shows the stored value: it changes when the value does, not before', async () => {
    const mounted = mount('checkbox', '');
    await fireEvent.click(box());
    expect(box()).not.toBeChecked();
    await mounted.setValue('true');
    expect(box()).toBeChecked();
  });

  it('is pressed with the keyboard like any button', async () => {
    const { onChange } = mount('checkbox', '');
    box().focus();
    expect(box()).toHaveFocus();
    // Space and Enter on a focused button arrive as a click.
    await fireEvent.click(box());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('true');
  });

  it('has no placeholder: an empty value is an unchecked box', () => {
    const { container } = mount('checkbox', '');
    expect(container).not.toHaveTextContent('Empty');
  });
});

describe('a checkbox property that may not be edited', () => {
  it('shows its state and does nothing when pressed', async () => {
    const { onChange } = mount('checkbox', 'true', { readOnly: true });
    expect(box()).toBeChecked();
    expect(box()).toBeDisabled();
    await fireEvent.click(box());
    expect(onChange).not.toHaveBeenCalled();
    expect(box()).toBeChecked();
  });

  it('is the same in display mode', async () => {
    const { onChange } = mount('checkbox', '', { mode: 'display' });
    expect(box()).toBeDisabled();
    await fireEvent.click(box());
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('a checkbox property asked to start in the editing state', () => {
  it('has no editing state: it is the same box', async () => {
    const { onChange } = mount('checkbox', '', { startOpen: true });
    expect(box()).not.toBeChecked();
    await fireEvent.click(box());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('true');
  });
});
