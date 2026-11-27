/** The names of the reader's organizations, read for the organizations their workspaces are in. */
import { describe, expect, it, vi } from 'vitest';

import { OrganizationNames, type WorkspaceInfo } from '$lib/state/index.ts';

const inOrg = (id: string, orgId: string): WorkspaceInfo => ({ id, orgId, title: id });

describe('the names of the organizations', () => {
  it('are read once for the same organizations, and again when they change', async () => {
    const read = vi.fn(() =>
      Promise.resolve([{ id: 'o1', name: 'Acme', role: 'member' as const }]),
    );
    const names = new OrganizationNames(read);
    expect(names.of('o1')).toBe('');
    names.follow([inOrg('a', 'o1')]);
    names.follow([inOrg('b', 'o1'), inOrg('a', 'o1')]);
    await vi.waitFor(() => {
      expect(names.of('o1')).toBe('Acme');
    });
    expect(read).toHaveBeenCalledOnce();
    names.follow([inOrg('a', 'o1'), inOrg('c', 'o2')]);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('are not read for a list without organizations, and leave names unknown on a failure', async () => {
    const read = vi.fn(() => Promise.reject(new Error('offline')));
    const names = new OrganizationNames(read);
    names.follow([{ id: 'own', kind: 'personal', title: '' }]);
    expect(read).not.toHaveBeenCalled();
    names.follow([inOrg('a', 'o1')]);
    await vi.waitFor(() => {
      expect(read).toHaveBeenCalledOnce();
    });
    expect(names.of('o1')).toBe('');
    // A failed read is tried again the next time.
    names.follow([inOrg('a', 'o1')]);
    await vi.waitFor(() => {
      expect(read).toHaveBeenCalledTimes(2);
    });
  });
});
