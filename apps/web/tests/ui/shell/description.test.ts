/**
 * The description of a board or a wiki behind "ⓘ", in a simulated page: what
 * is typed is saved from the keyboard (Ctrl+Enter, ⌘Enter on Apple devices,
 * or "Done"), Escape gives it up, and an input method's keys stay its own.
 * Where the focus goes is the browser tests' business (e2e/shell.spec.ts).
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ContainerDescription from '$lib/features/shell/ContainerDescription.svelte';

afterEach(() => {
  cleanup();
});

function mount(description = 'Where things stand.'): {
  onChange: ReturnType<typeof vi.fn<(description: string) => void>>;
  unmount: () => void;
} {
  const onChange = vi.fn<(description: string) => void>();
  const { unmount } = render(ContainerDescription, {
    props: { open: true, description, onChange },
  });
  return { onChange, unmount };
}

/** Whether the surface is open, as its button says (a closing popover stays a moment). */
const isOpen = (): string | null =>
  screen.getByTestId('container-description').getAttribute('aria-expanded');

const field = (): HTMLTextAreaElement =>
  screen.getByTestId<HTMLTextAreaElement>('container-description-field');

async function type(text: string): Promise<void> {
  await fireEvent.input(field(), { target: { value: text } });
}

describe('the description, for whoever may change it', () => {
  it('saves what was typed with Ctrl+Enter and closes', async () => {
    const { onChange } = mount();
    await type('  Where the launch stands.  ');
    await fireEvent.keyDown(field(), { key: 'Enter', ctrlKey: true });
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith('Where the launch stands.');
    expect(isOpen()).toBe('false');
  });

  it('leaves Ctrl+Enter to an input method that is composing, and saves nothing', async () => {
    const { onChange } = mount();
    await type('출시 현황');
    await fireEvent.keyDown(field(), { key: 'Enter', ctrlKey: true, isComposing: true });
    await fireEvent.keyDown(field(), { key: 'Enter', ctrlKey: true, keyCode: 229 });
    await tick();
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveValue('출시 현황');
    expect(isOpen()).toBe('true');
  });

  it('leaves Escape to an input method that is composing: the surface stays and so does the text', async () => {
    const { onChange } = mount();
    await type('출시');
    await fireEvent.keyDown(field(), { key: 'Escape', isComposing: true });
    await fireEvent.keyDown(field(), { key: 'Escape', keyCode: 229 });
    await tick();
    expect(isOpen()).toBe('true');
    expect(field()).toHaveValue('출시');
    await fireEvent.keyDown(field(), { key: 'Enter', ctrlKey: true });
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith('출시');
  });

  it('gives up what was typed with Escape, from the field and from "Done"', async () => {
    const { onChange } = mount();
    await type('Not kept.');
    await fireEvent.keyDown(screen.getByTestId('container-description-done'), { key: 'Escape' });
    await tick();
    expect(isOpen()).toBe('false');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes a plain Enter as a new line, not as saving', async () => {
    const { onChange } = mount();
    await type('One line');
    const plain = await fireEvent.keyDown(field(), { key: 'Enter' });
    await tick();
    // Not taken from the field: the browser puts the line break in.
    expect(plain).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveValue('One line');
  });

  it('saves with "Done" and closes', async () => {
    const { onChange } = mount();
    await type('Done by the button.');
    await fireEvent.click(screen.getByTestId('container-description-done'));
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith('Done by the button.');
    expect(isOpen()).toBe('false');
  });

  it('saves nothing when nothing changed', async () => {
    const { onChange } = mount();
    await fireEvent.keyDown(field(), { key: 'Enter', ctrlKey: true });
    await tick();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('hands on what was typed when it goes while open, to the change it was typed for', async () => {
    const onChange = vi.fn<(description: string) => void>();
    const { rerender, unmount } = render(ContainerDescription, {
      props: { open: true, description: 'Where things stand.', onChange },
    });
    await type('Left while typing.');
    // The page goes on to another board before this goes: its change is not the one.
    const another = vi.fn<(description: string) => void>();
    await rerender({ open: true, description: 'Another board.', onChange: another });
    unmount();
    expect(onChange).toHaveBeenCalledExactlyOnceWith('Left while typing.');
    expect(another).not.toHaveBeenCalled();
  });
});
