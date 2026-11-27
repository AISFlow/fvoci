/**
 * Where a container may be copied to, and what is known of an organization
 * that keeps its containers in (`copy-out.svelte.ts`).
 */
import { describe, expect, it } from 'vitest';

import {
  copyPlaces,
  forgetCopyRefusal,
  keepsCopiesIn,
  noteCopyOut,
  type WorkspaceInfo,
} from '$lib/state/index.ts';

const place = (id: string, more: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  id,
  title: id,
  orgId: 'o',
  role: 'member',
  ...more,
});

describe('where a container may be copied to', () => {
  it('offers every other open workspace but those the reader is a guest of', () => {
    const workspaces = [
      place('here'),
      place('there'),
      place('guest', { role: 'guest' }),
      place('closed', { ssoRequired: true }),
      place('mine', { orgId: 'p', kind: 'personal', role: 'admin' }),
    ];
    expect(copyPlaces(workspaces, 'here').map((ws) => ws.id)).toEqual(['there', 'mine']);
  });
});

describe('an organization that keeps its containers in', () => {
  it('is tried again after a refusal is forgotten', () => {
    noteCopyOut('refusing', false, 'refused');
    expect(keepsCopiesIn('refusing')).toBe(true);
    forgetCopyRefusal('refusing');
    expect(keepsCopiesIn('refusing')).toBe(false);
  });

  it('holds what an administrator read through a refusal and its forgetting', () => {
    noteCopyOut('read', false);
    noteCopyOut('read', false, 'refused');
    forgetCopyRefusal('read');
    expect(keepsCopiesIn('read')).toBe(true);
    noteCopyOut('read', true);
    expect(keepsCopiesIn('read')).toBe(false);
  });
});
