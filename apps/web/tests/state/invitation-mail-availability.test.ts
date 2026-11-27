/** Whether mail can be sent is asked again after a failed read, and kept once known. */
import { afterEach, expect, it, vi } from 'vitest';

import { Http } from '$lib/api/http.ts';
import { ServerInvitationMail } from '$lib/api/invitation-mail.ts';

afterEach(() => {
  vi.unstubAllGlobals();
});

it('asks again after a failed read, then keeps the answer', async () => {
  const answers = [
    new Response(JSON.stringify({ status: 503, title: 'Service unavailable' }), {
      status: 503,
      headers: { 'Content-Type': 'application/problem+json' },
    }),
    new Response(JSON.stringify({ registrationOpen: false, mailEnabled: true }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }),
  ];
  const fetch = vi.fn(() =>
    Promise.resolve(answers.shift() ?? new Response(null, { status: 500 })),
  );
  vi.stubGlobal('fetch', fetch);
  const mail = new ServerInvitationMail(new Http(''));
  await expect(mail.available()).rejects.toThrow();
  await expect(mail.available()).resolves.toBe(true);
  await expect(mail.available()).resolves.toBe(true);
  expect(fetch).toHaveBeenCalledTimes(2);
});
