/** The shared sidebar's shortcut must leave a field's formatting and IME alone. */
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import Provider from '$lib/ui/sidebar/sidebar-provider.svelte';

afterEach(() => {
  cleanup();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

for (const [platform, modifier, other] of [
  ['MacIntel', { metaKey: true }, { ctrlKey: true }],
  ['Win32', { ctrlKey: true }, { metaKey: true }],
] as const) {
  it(`${platform}: the native modifier toggles once, while the other key and composition do not`, async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    const changed = vi.fn();
    render(Provider, { onOpenChange: changed });
    for (const init of [
      other,
      { ...modifier, repeat: true },
      { ...modifier, isComposing: true },
      { ...modifier, keyCode: 229 },
      { ...modifier, altKey: true },
      { ...modifier, shiftKey: true },
      { ctrlKey: true, metaKey: true },
    ]) {
      await fireEvent.keyDown(window, { key: 'b', ...init });
      expect(changed).not.toHaveBeenCalled();
    }
    await fireEvent.keyDown(window, { key: 'b', ...modifier });
    expect(changed).toHaveBeenCalledExactlyOnceWith(false);
  });

  it(`${platform}: fields and an already handled shortcut retain their own keys`, async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    const changed = vi.fn();
    render(Provider, { onOpenChange: changed });
    for (const tag of ['input', 'textarea', 'div']) {
      const field = document.createElement(tag);
      if (tag === 'div') field.setAttribute('contenteditable', 'true');
      document.body.append(field);
      await fireEvent.keyDown(field, { key: 'b', ...modifier });
      expect(changed).not.toHaveBeenCalled();
      field.remove();
    }
    const handled = new KeyboardEvent('keydown', { key: 'b', ...modifier, cancelable: true });
    handled.preventDefault();
    window.dispatchEvent(handled);
    expect(changed).not.toHaveBeenCalled();
  });
}
