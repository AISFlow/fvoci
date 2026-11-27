/**
 * The value of a URL property: a link with buttons to edit and to copy it,
 * or a field when there is no address yet or it is being edited.
 */
import { describe, expect, it, vi } from 'vitest';

import { fireEvent, leave, mount, press, screen, tick, type } from './support.ts';

const field = (): HTMLInputElement => screen.getByRole('textbox', { name: 'My url' });
const editButton = (): HTMLElement => screen.getByRole('button', { name: 'My url: Edit' });
const copyButton = (): HTMLElement => screen.getByRole('button', { name: /^My url: Cop/ });

describe('a URL property without a value', () => {
  it('is a field at once, with a placeholder', () => {
    mount('url', '');
    expect(field()).toHaveValue('');
    expect(field()).toHaveAttribute('placeholder', 'Empty');
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('saves a typed address and becomes a link once the value is stored', async () => {
    const mounted = mount('url', '');
    await type(field(), 'https://example.org/page');
    await press(field(), 'Enter');
    expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith('https://example.org/page');
    await mounted.setValue('https://example.org/page');
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByRole('link', { name: 'https://example.org/page' })).toHaveAttribute(
      'href',
      'https://example.org/page',
    );
  });

  it('saves on leaving the field', async () => {
    const { onChange } = mount('url', '');
    await type(field(), 'example.org');
    await leave(field());
    expect(onChange).toHaveBeenCalledExactlyOnceWith('example.org');
  });

  it('empties the field again on Escape and saves nothing', async () => {
    const { onChange } = mount('url', '');
    await type(field(), 'http://test');
    await press(field(), 'Escape');
    expect(field()).toHaveValue('');
    expect(onChange).not.toHaveBeenCalled();
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
  });

  it('does not save what cannot be an address', async () => {
    const { onChange } = mount('url', '');
    await type(field(), 'two words');
    await press(field(), 'Enter');
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
    expect(field()).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('Enter a link address');
  });

  it('asks a phone for the address keys', () => {
    mount('url', '');
    expect(field()).toHaveAttribute('inputmode', 'url');
  });
});

describe('a URL property with a value', () => {
  it('is a link that opens in a new tab without handing over the opener', () => {
    mount('url', 'https://example.org/page');
    const link = screen.getByRole('link', { name: 'https://example.org/page' });
    expect(link).toHaveAttribute('href', 'https://example.org/page');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('takes the Tab stop it is given, where many values share one', () => {
    mount('url', 'https://example.org/page', { innerTabindex: -1 });
    for (const control of [screen.getByRole('link'), editButton(), copyButton()]) {
      expect(control).toHaveAttribute('tabindex', '-1');
      expect(control).toHaveAttribute('data-inner');
    }
  });

  it('has stops of its own everywhere else', () => {
    mount('url', 'https://example.org/page');
    for (const control of [screen.getByRole('link'), editButton(), copyButton()]) {
      expect(control).not.toHaveAttribute('tabindex', '-1');
      expect(control).not.toHaveAttribute('data-inner');
    }
  });

  it('adds https:// to an address without a scheme', () => {
    mount('url', ' example.org/page ');
    expect(screen.getByRole('link')).toHaveAttribute('href', 'https://example.org/page');
  });

  it.each(['javascript://example.org/%0aalert(1)', 'ftp://example.org/file', 'data://text/html,x'])(
    'shows %s as text, not as a link',
    (address) => {
      mount('url', address);
      expect(screen.queryByRole('link')).toBeNull();
      expect(screen.getByText(address)).toBeInTheDocument();
    },
  );

  it('does not let a press on the link reach what is around it', async () => {
    mount('url', 'https://example.org');
    const outside = vi.fn();
    document.body.addEventListener('click', outside);
    const link = screen.getByRole('link');
    link.addEventListener('click', (event) => {
      event.preventDefault();
    });
    await fireEvent.click(link);
    document.body.removeEventListener('click', outside);
    expect(outside).not.toHaveBeenCalled();
  });

  it('turns into a focused field with the Edit button, and saves the changed address', async () => {
    const mounted = mount('url', 'https://example.org');
    await fireEvent.click(editButton());
    expect(field()).toHaveValue('https://example.org');
    expect(field()).toHaveFocus();
    await type(field(), 'https://example.org/changed');
    await press(field(), 'Enter');
    expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith('https://example.org/changed');
    await mounted.setValue('https://example.org/changed');
    expect(screen.getByRole('link', { name: 'https://example.org/changed' })).toBeInTheDocument();
  });

  it('goes back to the link on Escape, with the focus on the Edit button, and saves nothing', async () => {
    const { onChange } = mount('url', 'https://example.org');
    await fireEvent.click(editButton());
    await type(field(), 'https://elsewhere.example');
    await press(field(), 'Escape');
    await tick();
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByRole('link', { name: 'https://example.org' })).toBeInTheDocument();
    expect(editButton()).toHaveFocus();
  });

  it('goes back to the link when the field is left unchanged', async () => {
    const { onChange } = mount('url', 'https://example.org');
    await fireEvent.click(editButton());
    await leave(field());
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('link')).toBeInTheDocument();
  });

  it('stays a field when the address is removed', async () => {
    const mounted = mount('url', 'https://example.org');
    await fireEvent.click(editButton());
    await type(field(), '');
    await press(field(), 'Enter');
    expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith('');
    await mounted.setValue('');
    expect(field()).toHaveValue('');
    expect(screen.queryByRole('link')).toBeNull();
  });

  it('copies the address and says so', async () => {
    const copyText = vi.fn<(text: string) => void>();
    const { onChange } = mount('url', 'https://example.org', { copyText });
    await fireEvent.click(copyButton());
    await tick();
    expect(copyText).toHaveBeenCalledExactlyOnceWith('https://example.org');
    expect(screen.getByRole('status')).toHaveTextContent('Copied!');
    expect(screen.getByRole('button', { name: 'My url: Copied!' })).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('says nothing when the address could not be copied', async () => {
    const copyText = vi.fn<(text: string) => Promise<void>>().mockRejectedValue(new Error('no'));
    mount('url', 'https://example.org', { copyText });
    await fireEvent.click(copyButton());
    await tick();
    expect(screen.getByRole('status')).toHaveTextContent('');
  });
});

describe('a URL property that may not be edited', () => {
  it('keeps the link and the Copy button, without the Edit button', async () => {
    const copyText = vi.fn<(text: string) => void>();
    const { onChange } = mount('url', 'https://example.org', { readOnly: true, copyText });
    expect(screen.getByRole('link')).toHaveAttribute('href', 'https://example.org');
    expect(screen.queryByRole('button', { name: 'My url: Edit' })).toBeNull();
    await fireEvent.click(copyButton());
    expect(copyText).toHaveBeenCalledExactlyOnceWith('https://example.org');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is the same in display mode', () => {
    mount('url', 'https://example.org', { mode: 'display' });
    expect(screen.getByRole('link')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'My url: Edit' })).toBeNull();
  });

  it('shows the placeholder, and no field, without a value', () => {
    mount('url', '', { readOnly: true });
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByText('Empty')).toBeInTheDocument();
  });
});

describe('a URL property that starts in the editing state', () => {
  it('is a focused field holding the address', () => {
    mount('url', 'https://example.org', { startOpen: true });
    expect(field()).toHaveValue('https://example.org');
    expect(field()).toHaveFocus();
  });
});
