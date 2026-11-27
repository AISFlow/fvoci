/** The bar above an open wiki the reader has not joined: what joining gives, said by the role. */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import WikiJoin from '$lib/features/wiki/WikiJoin.svelte';

afterEach(cleanup);

it('says joining is for writing where the wiki gives editors, and the role where it gives less', () => {
  const onJoin = vi.fn();
  const { unmount } = render(WikiJoin, { joining: false, role: 'editor', onJoin });
  expect(screen.getByTestId('wiki-join')).toHaveTextContent('Join it to write in it.');
  unmount();
  render(WikiJoin, { joining: false, role: 'viewer', onJoin });
  expect(screen.getByTestId('wiki-join')).toHaveTextContent('Joining makes you one, as Viewer.');
});

it('joins once, the hint describing the button, and not again while it joins', async () => {
  const onJoin = vi.fn();
  const { rerender } = render(WikiJoin, { joining: false, role: 'commenter', onJoin });
  const button = screen.getByRole('button', { name: 'Join this wiki' });
  expect(button).toHaveAccessibleDescription(/as Commenter/);
  await fireEvent.click(button);
  expect(onJoin).toHaveBeenCalledTimes(1);
  await rerender({ joining: true, role: 'commenter', onJoin });
  await fireEvent.click(button);
  expect(onJoin).toHaveBeenCalledTimes(1);
});
