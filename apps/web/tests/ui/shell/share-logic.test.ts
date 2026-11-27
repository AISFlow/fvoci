/** The share dialog's roles and public link. */
import { describe, expect, it } from 'vitest';

import { roleOf } from '$lib/state/board-parts.ts';
import type { ContainerRole } from '$lib/state/index.ts';

import { roleOfMember } from '$lib/features/shell/share-logic.ts';

import { routePath } from '../../../src/app/router.svelte.ts';

const member = (role: ContainerRole) => ({
  containerId: 'b',
  userId: 'u',
  role,
  synthetic: false,
});

describe('the share dialog', () => {
  it('names the direct role of a membership', () => {
    expect(roleOfMember(member('admin'))).toBe('admin');
    expect(roleOfMember(member('editor'))).toBe('editor');
    expect(roleOfMember(member('commenter'))).toBe('commenter');
    expect(roleOfMember(member('viewer'))).toBe('viewer');
  });

  it('keeps direct role controls separate from the highest inherited access', () => {
    const inherited = { ...member('viewer'), inheritedRole: 'editor' as const };
    expect(roleOfMember(inherited)).toBe('viewer');
    expect(roleOf(inherited)).toBe('editor');
    expect(roleOf({ ...inherited, role: 'admin' })).toBe('admin');
    expect(roleOf({ ...member('editor'), inheritedRole: 'commenter' })).toBe('editor');
    expect(roleOf({ ...member('viewer'), inheritedRole: 'viewer', synthetic: true })).toBe(
      'viewer',
    );
    expect(roleOf({ ...member('editor'), inheritedRole: 'viewer', synthetic: true })).toBe(
      'editor',
    );
  });

  it('makes the public link the reader’s address of the board, the read token in it', () => {
    expect(routePath({ boardId: 'b1', readToken: 'a+b' }, 'app')).toBe('/app/shared/b1?r=a%2Bb');
  });
});
