/**
 * Starting an organization from the workspace switcher: the workspace made comes into
 * the reader's list as the backend orders it, and a refusal is said by what
 * the server tells, the number an account may start included, which the client does not know.
 */
import { describe, expect, it, vi } from 'vitest';

import { newOrganizationProblemOf } from '$lib/features/shell/new-organization.ts';
import {
  BackendError,
  startOrganization,
  type Backend,
  type WorkspaceInfo,
} from '$lib/state/index.ts';

const own: WorkspaceInfo = { id: 'own', title: '', kind: 'personal', role: 'admin' };
const made: WorkspaceInfo = {
  id: 'made',
  title: 'Made',
  kind: 'shared',
  role: 'admin',
  orgId: 'o',
};

function holder(read: () => Promise<WorkspaceInfo[]>) {
  return { workspaces: [own], backend: { workspaces: read } as unknown as Backend };
}

describe('starting an organization', () => {
  it('reads the reader’s workspaces again, in the backend’s order, and answers the one made', async () => {
    const shown = holder(() => Promise.resolve([made, own]));
    const start = vi.fn(() => Promise.resolve(made));
    expect(await startOrganization(shown, start, 'Made')).toEqual(made);
    expect(start).toHaveBeenCalledWith('Made');
    expect(shown.workspaces).toEqual([made, own]);
  });

  it('puts the one made among the workspaces when the list cannot be read', async () => {
    const shown = holder(() => Promise.reject(new BackendError(0)));
    await startOrganization(shown, () => Promise.resolve(made), 'Made');
    expect(shown.workspaces).toEqual([own, made]);
  });

  it('changes nothing when it is refused', async () => {
    const read = vi.fn(() => Promise.resolve([made, own]));
    const shown = holder(read);
    const refusal = new BackendError(403, { reason: 'invitation_required' });
    await expect(startOrganization(shown, () => Promise.reject(refusal), 'Made')).rejects.toBe(
      refusal,
    );
    expect(read).not.toHaveBeenCalled();
    expect(shown.workspaces).toEqual([own]);
  });
});

describe('what the dialog says of a refusal', () => {
  it('names the limit the server tells, and no other', () => {
    const two = newOrganizationProblemOf(
      new BackendError(400, { reason: 'too_many_organizations', limit: 2 }),
    );
    const five = newOrganizationProblemOf(
      new BackendError(400, { reason: 'too_many_organizations', limit: 5 }),
    );
    expect(two.ofName).toBe(false);
    expect(two.text).toContain('2');
    expect(five.text).toContain('5');
    expect(five.text).not.toBe(two.text);
  });

  it('tells a server that takes one organization apart from a failure', () => {
    const closed = newOrganizationProblemOf(
      new BackendError(403, { reason: 'invitation_required' }),
    );
    const failed = newOrganizationProblemOf(new BackendError(500));
    expect(closed.ofName).toBe(false);
    expect(closed.text).not.toBe(failed.text);
    expect(newOrganizationProblemOf(new Error('no answer')).text).toBe(failed.text);
  });

  it('says what is wrong with the name beside the field', () => {
    const errors = [{ pointer: '/name', reason: 'too_long', detail: 'too long' }];
    const long = newOrganizationProblemOf(new BackendError(400, { errors }));
    expect(long.ofName).toBe(true);
    const odd = newOrganizationProblemOf(
      new BackendError(400, {
        errors: [{ pointer: '/name', reason: 'invalid_characters', detail: 'odd' }],
      }),
    );
    expect(odd.ofName).toBe(true);
    expect(odd.text).not.toBe(long.text);
  });

  it('says that there were too many tries', () => {
    const many = newOrganizationProblemOf(new BackendError(429));
    expect(many.text).not.toBe(newOrganizationProblemOf(new BackendError(500)).text);
  });
});
