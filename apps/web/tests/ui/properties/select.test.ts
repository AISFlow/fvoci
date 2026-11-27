/**
 * The value of a select property: one option, picked from a list that can
 * be searched and added to; and the settings of an option (name, colour,
 * deleting), which are reported to the caller.
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

const button = (): HTMLElement => valueButton('My select');
const search = (): HTMLInputElement => screen.getByRole('combobox', { name: 'Value selector' });
const list = (): HTMLElement => screen.getByRole('listbox', { name: 'Value selector' });
const option = (name: string | RegExp): HTMLElement => within(list()).getByRole('option', { name });
const optionNames = (): (string | null)[] =>
  within(list())
    .getAllByRole('option')
    .map((row) => row.textContent.trim());
const isOpen = (): boolean => button().getAttribute('aria-expanded') === 'true';

async function open(): Promise<void> {
  await fireEvent.click(button());
}

describe('a select property, closed', () => {
  it('shows the selected option under the name of the property', () => {
    mount('select', 'option-2');
    expect(button()).toHaveAccessibleName('My select In progress');
    expect(within(button()).getByText('In progress').closest('[data-color]')).toHaveAttribute(
      'data-color',
      'blue',
    );
  });

  it('shows a placeholder without a value, when asked to', async () => {
    const mounted = mount('select', '');
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(button()).toHaveAccessibleName('My select');
  });

  it('shows nothing for a value whose option is gone', () => {
    mount('select', 'deleted-option');
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
  });

  it('does not show the list', () => {
    mount('select', 'option-2');
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(isOpen()).toBe(false);
  });
});

describe('a select property, opened', () => {
  it('lists the options when the value is pressed', async () => {
    mount('select', 'option-2');
    await open();
    expect(isOpen()).toBe(true);
    expect(optionNames()).toEqual(['To do', 'In progress', 'Done']);
    expect(option('In progress')).toHaveAttribute('aria-selected', 'true');
    expect(option('Done')).toHaveAttribute('aria-selected', 'false');
  });

  it('stores the option that is pressed, once, and closes', async () => {
    const { onChange } = mount('select', 'option-2');
    await open();
    await fireEvent.click(option('Done'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-3');
    expect(isOpen()).toBe(false);
  });

  it('closes without storing when the selected option is pressed again', async () => {
    const { onChange } = mount('select', 'option-2');
    await open();
    await fireEvent.click(option('In progress'));
    expect(onChange).not.toHaveBeenCalled();
    expect(isOpen()).toBe(false);
  });

  it('clears the value', async () => {
    const { onChange } = mount('select', 'option-2');
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
    expect(isOpen()).toBe(false);
  });

  it('offers nothing to clear without a value', async () => {
    mount('select', '');
    await open();
    expect(screen.queryByRole('button', { name: 'Clear' })).toBeNull();
  });

  it('closes on Escape and stores nothing', async () => {
    const { onChange } = mount('select', 'option-2');
    await open();
    await closePicker();
    expect(isOpen()).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('narrows the list as a person types, without regard to case', async () => {
    mount('select', '');
    await open();
    await type(search(), 'PROG');
    expect(optionNames()).toEqual(['In progress']);
    await type(search(), 'do');
    expect(optionNames()).toEqual(['To do', 'Done']);
  });

  it('narrows a list of Korean names by typed Hangul', async () => {
    const options: PropertyOption[] = [
      { id: 'a', label: '진행 중', color: 'blue' },
      { id: 'b', label: '완료', color: 'green' },
    ];
    const { onChange } = mount(templateOf('select', { options }), '');
    await open();
    await type(search(), '완');
    expect(optionNames()).toEqual(['완료']);
    // The Enter that confirms a syllable is not the Enter that picks.
    await press(search(), 'Enter', { isComposing: true });
    expect(onChange).not.toHaveBeenCalled();
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('b');
  });

  it('picks with the keyboard: arrows move the highlight, Enter picks', async () => {
    const { onChange } = mount('select', '');
    await open();
    expect(search()).toHaveAttribute('aria-activedescendant', option('To do').id);
    await press(search(), 'ArrowDown');
    expect(search()).toHaveAttribute('aria-activedescendant', option('In progress').id);
    await press(search(), 'ArrowDown');
    await press(search(), 'ArrowDown');
    // Past the last row the highlight starts over.
    expect(search()).toHaveAttribute('aria-activedescendant', option('To do').id);
    await press(search(), 'ArrowUp');
    expect(search()).toHaveAttribute('aria-activedescendant', option('Done').id);
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-3');
    expect(isOpen()).toBe(false);
  });

  it('picks the first match on Enter after typing', async () => {
    const { onChange } = mount('select', '');
    await open();
    await type(search(), 'don');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-3');
  });

  it('starts with an empty field every time it opens', async () => {
    mount('select', '');
    await open();
    await type(search(), 'prog');
    await closePicker();
    await open();
    expect(search()).toHaveValue('');
    expect(optionNames()).toEqual(['To do', 'In progress', 'Done']);
  });
});

describe('creating an option from a select property', () => {
  const created: PropertyOption = { id: 'option-new', label: 'Blocked', color: 'default' };

  it('offers to create a name that no option has', async () => {
    mount('select', '', { onCreateOption: () => created });
    await open();
    await type(search(), 'Blocked');
    expect(optionNames()).toEqual(['Create “Blocked”']);
  });

  it('does not offer to create a name an option already has, whatever its case', async () => {
    mount('select', '', { onCreateOption: () => created });
    await open();
    await type(search(), 'done');
    expect(optionNames()).toEqual(['Done']);
  });

  it('asks the caller for the option, then stores it as the value and closes', async () => {
    const onCreateOption = vi.fn((_name: string) => created);
    const { onChange } = mount('select', 'option-1', { onCreateOption });
    await open();
    await type(search(), '  Blocked ');
    await fireEvent.click(option('Create “Blocked”'));
    await tick();
    expect(onCreateOption).toHaveBeenCalledExactlyOnceWith('Blocked');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-new');
    expect(isOpen()).toBe(false);
  });

  it('creates on Enter when nothing matches', async () => {
    const onCreateOption = vi.fn((_name: string) => created);
    const { onChange } = mount('select', '', { onCreateOption });
    await open();
    await type(search(), 'Blocked');
    await press(search(), 'Enter');
    await tick();
    expect(onCreateOption).toHaveBeenCalledExactlyOnceWith('Blocked');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-new');
  });

  it('waits for an option that takes a while to be made', async () => {
    let finish: (option: PropertyOption) => void = () => undefined;
    const onCreateOption = vi.fn(
      (_name: string) => new Promise<PropertyOption>((resolve) => (finish = resolve)),
    );
    const { onChange } = mount('select', '', { onCreateOption });
    await open();
    await type(search(), 'Blocked');
    await press(search(), 'Enter');
    expect(onChange).not.toHaveBeenCalled();
    finish(created);
    await tick();
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-new');
  });

  it('leaves the value alone when the option could not be made', async () => {
    const onCreateOption = vi.fn(async (_name: string): Promise<PropertyOption> => {
      throw new Error('refused');
    });
    const { onChange } = mount('select', 'option-1', { onCreateOption });
    await open();
    await type(search(), 'Blocked');
    await press(search(), 'Enter');
    await tick();
    await tick();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('cannot create without a way to make options', async () => {
    mount('select', '');
    await open();
    await type(search(), 'Blocked');
    expect(within(list()).queryAllByRole('option')).toHaveLength(0);
    expect(screen.getByRole('status')).toHaveTextContent('No matches');
  });

  it('invites to type when there are no options yet', async () => {
    mount(templateOf('select', { options: [] }), '', { onCreateOption: () => created });
    await open();
    expect(screen.getByRole('status')).toHaveTextContent(
      'No options. Start typing to add the first one!',
    );
  });
});

describe('the settings of an option', () => {
  const callbacks = () => ({
    onRenameOption: vi.fn<(option: PropertyOption, name: string) => void>(),
    onRecolorOption: vi.fn<(option: PropertyOption, color: string) => void>(),
    onDeleteOption: vi.fn<(option: PropertyOption) => void>(),
  });
  const settingsButton = (name: string): HTMLElement =>
    screen.getByRole('button', { name: `Edit option “${name}”` });
  const nameField = (): HTMLInputElement => screen.getByRole('textbox', { name: 'Option name' });

  it('are not offered when the caller takes no changes to options', async () => {
    mount('select', '');
    await open();
    expect(screen.queryByRole('button', { name: /^Edit option/ })).toBeNull();
  });

  it('open in place of the list and go back to it', async () => {
    mount('select', '', callbacks());
    await open();
    await fireEvent.click(settingsButton('Done'));
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(nameField()).toHaveValue('Done');
    expect(screen.getByRole('radio', { name: 'Green' })).toBeChecked();
    await fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    await tick();
    expect(optionNames()).toEqual(['To do', 'In progress', 'Done']);
    expect(settingsButton('Done')).toHaveFocus();
  });

  it('report a new name on Enter, and do not touch the value', async () => {
    const reported = callbacks();
    const { onChange } = mount('select', 'option-3', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    await type(nameField(), ' Shipped ');
    await press(nameField(), 'Enter');
    expect(reported.onRenameOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[2], 'Shipped');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('do not report a name that is empty or unchanged', async () => {
    const reported = callbacks();
    mount('select', '', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    await type(nameField(), '   ');
    await press(nameField(), 'Enter');
    expect(nameField()).toHaveValue('Done');
    await type(nameField(), 'Done');
    await press(nameField(), 'Enter');
    expect(reported.onRenameOption).not.toHaveBeenCalled();
  });

  it('put the name back on the first Escape and leave the picker open', async () => {
    const reported = callbacks();
    mount('select', '', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    await type(nameField(), 'Shipp');
    await press(nameField(), 'Escape');
    expect(nameField()).toHaveValue('Done');
    expect(isOpen()).toBe(true);
    expect(reported.onRenameOption).not.toHaveBeenCalled();
  });

  it('offer the ten colours and report the one chosen', async () => {
    const reported = callbacks();
    mount('select', '', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    expect(screen.getAllByRole('radio').map((radio) => radio.textContent.trim())).toEqual([
      'Default',
      'Gray',
      'Brown',
      'Orange',
      'Yellow',
      'Green',
      'Blue',
      'Purple',
      'Pink',
      'Red',
    ]);
    await fireEvent.click(screen.getByRole('radio', { name: 'Brown' }));
    expect(reported.onRecolorOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[2], 'brown');
    // The colour it already has is not reported again.
    await fireEvent.click(screen.getByRole('radio', { name: 'Green' }));
    expect(reported.onRecolorOption).toHaveBeenCalledTimes(1);
  });

  it('report a deleted option and go back to the list', async () => {
    const reported = callbacks();
    const { onChange } = mount('select', 'option-3', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(reported.onDeleteOption).toHaveBeenCalledExactlyOnceWith(OPTIONS[2]);
    expect(onChange).not.toHaveBeenCalled();
    expect(list()).toBeInTheDocument();
  });

  it('show only what the caller takes', async () => {
    mount('select', '', { onDeleteOption: vi.fn() });
    await open();
    await fireEvent.click(settingsButton('Done'));
    expect(nameField()).toHaveAttribute('readonly');
    expect(screen.queryByRole('radiogroup')).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeInTheDocument();
  });

  it('follow the option as the caller changes it', async () => {
    const reported = callbacks();
    const mounted = mount('select', '', reported);
    await open();
    await fireEvent.click(settingsButton('Done'));
    const options = OPTIONS.map((existing) =>
      existing.id === 'option-3' ? { ...existing, color: 'red' as const } : existing,
    );
    await mounted.setProps({ template: templateOf('select', { options }) });
    expect(screen.getByRole('radio', { name: 'Red' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Green' })).not.toBeChecked();
  });
});

describe('a select property with many options', () => {
  const options: PropertyOption[] = Array.from({ length: 200 }, (_, index) => ({
    id: `option-${index}`,
    label: `Option ${index}`,
    color: 'default',
  }));

  it('lists them all and finds one by typing', async () => {
    const { onChange } = mount(templateOf('select', { options }), 'option-7');
    await open();
    expect(within(list()).getAllByRole('option')).toHaveLength(200);
    await type(search(), 'option 199');
    expect(optionNames()).toEqual(['Option 199']);
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-199');
  });
});

describe('a select property that may not be edited', () => {
  it('shows the option and cannot be opened when read only', () => {
    const { container } = mount('select', 'option-2', { readOnly: true });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('In progress');
  });

  it('is the same in display mode', () => {
    const { container, onChange } = mount('select', 'option-2', { mode: 'display' });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('In progress');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('does not open even when asked to start open', () => {
    mount('select', 'option-2', { readOnly: true, startOpen: true });
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('shows the placeholder, or nothing, without a value', async () => {
    const mounted = mount('select', '', { readOnly: true });
    expect(mounted.container).toHaveTextContent('Empty');
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(mounted.container).toHaveTextContent('');
  });
});

describe('a select property that starts in the editing state', () => {
  it('has its list open at once', () => {
    mount('select', 'option-2', { startOpen: true });
    expect(optionNames()).toEqual(['To do', 'In progress', 'Done']);
  });
});
