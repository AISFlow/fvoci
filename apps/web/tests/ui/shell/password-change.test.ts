/**
 * What the form that changes the password says about each refusal, and at
 * which field.
 */
import { describe, expect, it } from 'vitest';

import { passwordProblemOf } from '$lib/features/shell/password-change.ts';
import { refusedMember } from '$lib/state/backend.ts';
import { BackendError } from '$lib/state/index.ts';

const refusal = (status: 400 | 409 | 429 | 500, extra: object = {}) =>
  new BackendError(status, { detail: 'no', ...extra });

describe('a refused change of the password', () => {
  it('points at the current password when it is wrong', () => {
    expect(
      passwordProblemOf(
        refusal(400, {
          reason: 'invalid_credentials',
          errors: [refusedMember('oldPassword', 'wrong', 'invalid_credentials')],
        }),
        'long enough',
      ),
    ).toEqual({ text: 'The current password is wrong.', field: 'current' });
  });

  it('tells a new password that is too short from one that is too long, by the server’s measures', () => {
    // Told by the member `errors` points at, with the server's reason.
    const tooShort = refusal(400, { errors: [refusedMember('newPassword', 'no', 'too_short')] });
    expect(passwordProblemOf(tooShort, 'short')).toEqual({
      text: 'The new password needs at least 8 characters.',
      field: 'next',
    });
    // A reason this page does not know: its measures decide. Seven characters,
    // though fourteen UTF-16 units, are too short, as the server counts.
    const measured = refusal(400, { errors: [refusedMember('newPassword', 'no')] });
    expect(passwordProblemOf(measured, '😀😀😀😀😀😀😀').text).toBe(
      'The new password needs at least 8 characters.',
    );
    expect(passwordProblemOf(measured, '가'.repeat(342))).toEqual({
      text: 'The new password is too long.',
      field: 'next',
    });
    const tooLong = refusal(400, { errors: [refusedMember('newPassword', 'no', 'too_long')] });
    expect(passwordProblemOf(tooLong, 'long enough').text).toBe('The new password is too long.');
    // The reason decides, whatever this page measured.
    expect(passwordProblemOf(tooShort, 'long enough').text).toBe(
      'The new password needs at least 8 characters.',
    );
  });

  it('says what to do after a change elsewhere, too many tries, or anything else', () => {
    expect(passwordProblemOf(refusal(409), 'long enough').text).toBe(
      'The password was changed somewhere else meanwhile. Sign in again with the new one.',
    );
    expect(passwordProblemOf(refusal(429), 'long enough').text).toBe(
      'Too many tries. Wait a moment and try again.',
    );
    expect(passwordProblemOf(refusal(500), 'long enough')).toEqual({
      text: 'That did not work. Try again.',
      field: undefined,
    });
    expect(passwordProblemOf(new Error('bug'), 'long enough').field).toBeUndefined();
  });
});
