/**
 * The value of a multi-select property: several options. The list stays
 * open while options are added and taken out.
 */
import { describe, expect, it, vi } from 'vitest';

import type { PropertyOption } from '$lib/logic/wire.ts';

import {
  closePicker,
  fireEvent,
  mount,
  OPTIONS,
  press,
  screen,
  templateOf,
  tick,
  type,
  valueButton,
  within,
} from './support.ts';

const button = (): HTMLElement => valueButton('My multiSelect');
const search = (): HTMLInputElement => screen.getByRole('combobox', { name: 'Value selector' });
const list = (): HTMLElement => screen.getByRole('listbox', { name: 'Value selector' });
const option = (name: string | RegExp): HTMLElement => within(list()).getByRole('option', { name });
const isOpen = (): boolean => button().getAttribute('aria-expanded') === 'true';
const chips = (): (string | null)[] =>
  [...button().querySelectorAll('[data-slot="option-chip"]')].map((chip) =>
    chip.textContent.trim(),
  );

async function open(): Promise<void> {
  await fireEvent.click(button());
}

describe('a multi-select property, closed', () => {
  it('shows only the selected options, in the order of the value', () => {
    mount('multiSelect', ['option-3', 'option-1']);
    expect(chips()).toEqual(['Done', 'To do']);
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('shows a placeholder without a value', () => {
    mount('multiSelect', []);
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
  });

  it('does not show options that are gone, or the same option twice', () => {
    mount('multiSelect', ['option-1', 'deleted', 'option-1']);
    expect(chips()).toEqual(['To do']);
  });

  it('is empty for a value that is not a list', () => {
    mount('multiSelect', 'option-1');
    expect(chips()).toEqual([]);
  });
});

describe('a multi-select property, opened', () => {
  it('lists every option and marks the selected ones', async () => {
    mount('multiSelect', ['option-1']);
    await open();
    expect(list()).toHaveAttribute('aria-multiselectable', 'true');
    expect(option('To do')).toHaveAttribute('aria-selected', 'true');
    expect(option('In progress')).toHaveAttribute('aria-selected', 'false');
    expect(option('Done')).toHaveAttribute('aria-selected', 'false');
  });

  it('adds the option that is pressed and stays open', async () => {
    const { onChange } = mount('multiSelect', ['option-1']);
    await open();
    await fireEvent.click(option('Done'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-1', 'option-3']);
    expect(isOpen()).toBe(true);
  });

  it('adds one option after another as the value is stored', async () => {
    const mounted = mount('multiSelect', []);
    await open();
    await fireEvent.click(option('Done'));
    await mounted.setValue(['option-3']);
    await fireEvent.click(option('To do'));
    expect(mounted.onChange.mock.calls).toEqual([[['option-3']], [['option-3', 'option-1']]]);
  });

  it('takes out a selected option that is pressed in the list', async () => {
    const { onChange } = mount('multiSelect', ['option-1', 'option-3']);
    await open();
    await fireEvent.click(option('To do'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-3']);
  });

  it('takes out an option with the button on its label', async () => {
    const { onChange } = mount('multiSelect', ['option-1', 'option-3']);
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove To do' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-3']);
    expect(isOpen()).toBe(true);
  });

  it('takes out the last option on Backspace in the empty field', async () => {
    const { onChange } = mount('multiSelect', ['option-1', 'option-3']);
    await open();
    await press(search(), 'Backspace');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-1']);
  });

  it('leaves the value alone on Backspace while there is text to delete', async () => {
    const { onChange } = mount('multiSelect', ['option-1']);
    await open();
    await type(search(), 'd');
    await press(search(), 'Backspace');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('drops identifiers of options that are gone when the value is next changed', async () => {
    const { onChange } = mount('multiSelect', ['option-1', 'deleted']);
    await open();
    await fireEvent.click(option('Done'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-1', 'option-3']);
  });

  it('clears the value and stays open', async () => {
    const { onChange } = mount('multiSelect', ['option-1', 'option-3']);
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith([]);
    expect(isOpen()).toBe(true);
  });

  it('closes on Escape and stores nothing more', async () => {
    const { onChange } = mount('multiSelect', ['option-1']);
    await open();
    await closePicker();
    expect(isOpen()).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('picks with the keyboard and empties the field for the next pick', async () => {
    const { onChange } = mount('multiSelect', []);
    await open();
    await type(search(), 'prog');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-2']);
    expect(search()).toHaveValue('');
    expect(isOpen()).toBe(true);
  });
});

describe('creating an option from a multi-select property', () => {
  const created: PropertyOption = { id: 'option-new', label: 'Blocked', color: 'default' };

  it('asks the caller for the option and adds it to the value', async () => {
    const onCreateOption = vi.fn((_name: string) => created);
    const { onChange } = mount('multiSelect', ['option-1'], { onCreateOption });
    await open();
    await type(search(), 'Blocked');
    await fireEvent.click(option('Create “Blocked”'));
    await tick();
    expect(onCreateOption).toHaveBeenCalledExactlyOnceWith('Blocked');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['option-1', 'option-new']);
    expect(isOpen()).toBe(true);
    expect(search()).toHaveValue('');
  });
});

describe('the settings of an option of a multi-select property', () => {
  const settingsButton = (name: string): HTMLElement =>
    screen.getByRole('button', { name: `Edit option “${name}”` });

  it('report a deleted option', async () => {
    const onDeleteOption = vi.fn<(option: PropertyOption) => void>();
    const { onChange } = mount('multiSelect', ['option-1'], { onDeleteOption });
    await open();
    await fireEvent.click(settingsButton('To do'));
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(onDeleteOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[0]);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('report a changed colour for any option, selected or not', async () => {
    const onRecolorOption = vi.fn<(option: PropertyOption, color: string) => void>();
    mount('multiSelect', ['option-1'], { onRecolorOption });
    await open();
    await fireEvent.click(settingsButton('In progress'));
    await fireEvent.click(screen.getByRole('radio', { name: 'Pink' }));
    expect(onRecolorOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[1], 'pink');
  });

  it('report a new name', async () => {
    const onRenameOption = vi.fn<(option: PropertyOption, name: string) => void>();
    mount('multiSelect', [], { onRenameOption });
    await open();
    await fireEvent.click(settingsButton('To do'));
    const name = screen.getByRole('textbox', { name: 'Option name' });
    await type(name, '할 일');
    name.blur();
    expect(onRenameOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[0], '할 일');
  });
});

describe('a multi-select property with many options', () => {
  const options: PropertyOption[] = Array.from({ length: 120 }, (_, index) => ({
    id: `option-${index}`,
    label: `Tag ${index}`,
    color: 'default',
  }));

  it('shows a long value and finds an option by typing', async () => {
    const value = options.slice(0, 30).map((existing) => existing.id);
    const { onChange } = mount(templateOf('multiSelect', { options }), value);
    expect(chips()).toHaveLength(30);
    await open();
    await type(search(), 'tag 119');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith([...value, 'option-119']);
  });
});

describe('a multi-select property that may not be edited', () => {
  it('shows the options and cannot be opened', () => {
    const { container, onChange } = mount('multiSelect', ['option-1', 'option-3'], {
      readOnly: true,
    });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('To do');
    expect(container).toHaveTextContent('Done');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is the same in display mode, with nothing where there is no value', () => {
    const { container } = mount('multiSelect', [], {
      mode: 'display',
      showEmptyPlaceholder: false,
    });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('');
  });
});

describe('a multi-select property that starts in the editing state', () => {
  it('has its list open at once', () => {
    mount('multiSelect', [], { startOpen: true });
    expect(within(list()).getAllByRole('option')).toHaveLength(3);
  });
});
