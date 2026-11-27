/** Returning from the provider must leave the connection form usable again. */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import LinkedSignIns from '$lib/features/account/LinkedSignIns.svelte';

afterEach(cleanup);

it('can retry linking after Back restores the settings from the browser cache', async () => {
  const onLink = vi.fn(() => Promise.resolve());
  render(LinkedSignIns, {
    props: {
      read: () => Promise.resolve([]),
      organizations: () => Promise.resolve([{ orgId: 'org-a', name: 'Acme' }]),
      onLink,
      onUnlink: () => Promise.resolve(),
    },
  });
  await fireEvent.click(await screen.findByRole('button', { name: 'Connect single sign-on' }));
  await fireEvent.input(screen.getByLabelText('Current password'), {
    target: { value: 'the password' },
  });
  const submit = screen.getByRole('button', { name: 'Continue to single sign-on' });
  await fireEvent.click(submit);
  expect(onLink).toHaveBeenCalledExactlyOnceWith('org-a', 'the password');
  expect(submit).toBeDisabled();

  // The same page lifecycle event handled by SignInPage and SsoRequired.
  const shown = new Event('pageshow');
  Object.defineProperty(shown, 'persisted', { value: true });
  await fireEvent(window, shown);

  expect(submit).toBeEnabled();
  await fireEvent.click(submit);
  expect(onLink).toHaveBeenCalledTimes(2);
});
