/** How workspaces are named and grouped where the reader tells theirs apart. */
import { describe, expect, it } from 'vitest';

import { spaceLines, spaceName } from '$lib/features/shell/space-names.ts';
import type { WorkspaceInfo } from '$lib/state/index.ts';

const own: WorkspaceInfo = { id: 'own', kind: 'personal', title: '' };
const solo: WorkspaceInfo = { id: 'solo', orgId: 'o1', kind: 'shared', title: 'General' };
const first: WorkspaceInfo = { id: 'a', orgId: 'o2', kind: 'shared', title: 'Design' };
const second: WorkspaceInfo = { id: 'b', orgId: 'o2', kind: 'shared', title: 'Sales' };
const NAMES: Record<string, string> = { o1: 'Acme', o2: 'Globex' };
const orgName = (orgId: string | undefined): string => (orgId ? (NAMES[orgId] ?? '') : '');
const all = [solo, first, own, second];

describe('the name of a workspace', () => {
  it('is “My space” for the own space', () => {
    expect(spaceName(own, all, orgName)).toBe('My space');
  });

  it('is the organization’s alone where it has no other of the reader’s', () => {
    expect(spaceName(solo, all, orgName)).toBe('Acme');
  });

  it('is “organization · workspace” for one of several', () => {
    expect(spaceName(second, all, orgName)).toBe('Globex · Sales');
  });

  it('is its own while the organization’s name is not known', () => {
    expect(spaceName(solo, all, () => '')).toBe('General');
  });

  it('is said as its place says one without a title, where it goes by its own', () => {
    const untitled: WorkspaceInfo = { ...solo, title: '' };
    expect(
      spaceName(
        untitled,
        all,
        () => '',
        () => 'this workspace',
      ),
    ).toBe('this workspace');
    expect(spaceName(untitled, all, orgName, () => 'this workspace')).toBe('Acme');
  });
});

describe('the lines of the switcher', () => {
  it('puts the own space first, then each organization together, headed where it has several', () => {
    const lines = spaceLines(all, orgName);
    expect(lines.map((line) => [line.label, line.heading])).toEqual([
      ['My space', undefined],
      ['Acme', undefined],
      ['Design', 'Globex'],
      ['Sales', undefined],
    ]);
    expect(lines.map((line) => line.fullName)).toEqual([
      'My space',
      'Acme',
      'Globex · Design',
      'Globex · Sales',
    ]);
    expect(lines.map((line) => line.separated)).toEqual([false, false, true, false]);
  });

  it('sets the organization of one workspace that follows a group apart from it', () => {
    const lines = spaceLines([own, first, second, solo], orgName);
    expect(lines.map((line) => [line.label, line.separated])).toEqual([
      ['My space', false],
      ['Design', true],
      ['Sales', false],
      ['Acme', true],
    ]);
  });
});
