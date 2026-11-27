import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import InviteEmail from '$lib/features/shell/InviteEmail.svelte';
import { BackendError } from '$lib/state/index.ts';

afterEach(cleanup);

it('hides email delivery when SMTP is unavailable', async () => {
  const available = vi.fn(() => Promise.resolve(false));
  render(InviteEmail, {
    mail: { available, send: vi.fn() },
    workspaceId: 'workspace',
    token: 'secret',
    language: 'en',
    hint: 'Each person receives a link of their own.',
  });
  await waitFor(() => {
    expect(available).toHaveBeenCalledOnce();
  });
  expect(screen.queryByTestId('invite-email-form')).toBeNull();
});

it('sends addresses separately with the existing token and acknowledges queuing', async () => {
  const send = vi.fn(() => Promise.resolve());
  render(InviteEmail, {
    mail: { available: () => Promise.resolve(true), send },
    workspaceId: 'workspace',
    token: 'secret',
    language: 'ko',
    hint: 'Each person receives a link of their own.',
  });
  await fireEvent.input(await screen.findByLabelText('Email invitation to'), {
    target: { value: ' one@example.test,\ntwo@example.test ' },
  });
  await fireEvent.submit(screen.getByTestId('invite-email-form'));
  expect(send).toHaveBeenCalledExactlyOnceWith(
    'workspace',
    'secret',
    ['one@example.test', 'two@example.test'],
    'ko',
  );
  expect(await screen.findByRole('status')).toHaveTextContent('Invitation mail queued');
  expect(screen.getByLabelText('Email invitation to')).toHaveAccessibleDescription(
    'Each person receives a link of their own.',
  );
});

it('explains invalid addresses without showing server details', async () => {
  const send = vi.fn(() =>
    Promise.reject(
      new BackendError(400, {
        errors: [{ pointer: '/recipients', reason: 'invalid', detail: 'private detail' }],
      }),
    ),
  );
  render(InviteEmail, {
    mail: { available: () => Promise.resolve(true), send },
    workspaceId: 'workspace',
    token: 'secret',
    language: 'en',
    hint: 'Each person receives a link of their own.',
  });
  await fireEvent.input(await screen.findByLabelText('Email invitation to'), {
    target: { value: 'bad address' },
  });
  await fireEvent.submit(screen.getByTestId('invite-email-form'));
  expect(await screen.findByRole('alert')).toHaveTextContent('Enter valid email addresses');
  expect(screen.getByRole('alert')).not.toHaveTextContent('private detail');
});

it.each([
  [
    new BackendError(400, {
      reason: 'too_many_recipients',
      errors: [{ pointer: '/recipients', reason: 'too_many_recipients', detail: 'private detail' }],
    }),
    'Too many addresses for one sending',
  ],
  [
    new BackendError(400, {
      reason: 'too_many_recipients',
      limit: 20,
      errors: [{ pointer: '/recipients', reason: 'too_many_recipients', detail: 'private detail' }],
    }),
    'Send to at most 20 addresses at a time',
  ],
  [
    new BackendError(429, { reason: 'invitation_mail_limit', retryAfter: 3540 }),
    'try again in 59 min',
  ],
  [new BackendError(503, { reason: 'mail_queue_full' }), 'Too much mail is waiting'],
])('says why a whole request was refused: %s', async (refusal, said) => {
  const send = vi.fn(() => Promise.reject(refusal));
  render(InviteEmail, {
    mail: { available: () => Promise.resolve(true), send },
    workspaceId: 'workspace',
    token: 'secret',
    language: 'en',
    hint: 'Each person receives a link of their own.',
  });
  await fireEvent.input(await screen.findByLabelText('Email invitation to'), {
    target: { value: 'one@example.test' },
  });
  await fireEvent.submit(screen.getByTestId('invite-email-form'));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(said);
  expect(alert).toHaveTextContent('Nothing was sent');
  expect(alert).not.toHaveTextContent('private detail');
});
