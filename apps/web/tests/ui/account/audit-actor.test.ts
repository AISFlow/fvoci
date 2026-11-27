import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it } from 'vitest';
import AuditList from '$lib/components/audit/AuditList.svelte';
import type { AuditRecord } from '$lib/logic/audit.ts';

afterEach(cleanup);
it('distinguishes a server operator from a person whose account is gone', () => {
  const base = {
    action: 'sso_released',
    targetId: 'org',
    targetName: null,
    subject: null,
    createdAt: 1,
    detail: null,
    identity: null,
  } as const;
  const records: AuditRecord[] = [
    { ...base, id: 'operator', actor: { kind: 'server_operator' } },
    {
      ...base,
      id: 'person',
      actor: { kind: 'person', id: 'former', username: '', displayName: '' },
    },
  ];
  render(AuditList, { records, locale: 'en' });
  expect(screen.getByTestId('audit-list')).toHaveTextContent(
    "Server operator released the organization's single sign-on requirement",
  );
  expect(screen.getByTestId('audit-list')).toHaveTextContent(
    "Deleted user released the organization's single sign-on requirement",
  );
});

it('says what became of the previous provider, and whose identity was connected', () => {
  const person = { kind: 'person', id: 'ada', username: 'ada', displayName: '' } as const;
  const base = {
    targetId: 'org',
    targetName: null,
    subject: null,
    createdAt: 1,
    identity: null,
  } as const;
  const detail = 'https://before.example.test';
  const records: AuditRecord[] = [
    { ...base, id: 'kept', action: 'oidc_former_provider_kept', actor: person, detail },
    { ...base, id: 'ended', action: 'oidc_former_provider_ended', actor: person, detail },
    {
      ...base,
      id: 'lapsed',
      action: 'oidc_former_provider_lapsed',
      actor: { kind: 'server' },
      detail,
    },
    {
      ...base,
      id: 'bound',
      action: 'oidc_identity_bound',
      actor: person,
      detail,
      identity: 'ada@after.example.test',
    },
    {
      ...base,
      id: 'carried',
      action: 'oidc_identity_carried_over',
      actor: person,
      detail,
      identity: 'subject-of-ada',
    },
  ];
  render(AuditList, { records, locale: 'en' });
  const list = screen.getByTestId('audit-list');
  expect(list).toHaveTextContent(
    `@ada moved the sign-in provider; the previous one (${detail}) is kept for linking`,
  );
  expect(list).toHaveTextContent(
    `@ada ended the linking period of the previous provider (${detail}) early`,
  );
  expect(list).toHaveTextContent(`The linking period of the previous provider (${detail}) ran out`);
  expect(list).toHaveTextContent(
    `@ada connected their identity at the new provider (${detail}) to their account and put its settings in use`,
  );
  expect(list).toHaveTextContent(
    `@ada confirmed their previous sign-in and connected their identity at the new provider (${detail}) to their account`,
  );
  const identities = screen.getAllByTestId('audit-identity').map((line) => line.textContent);
  expect(identities).toEqual([
    'Identity connected: ada@after.example.test (as the new provider gave it, unchecked)',
    'Identity connected: subject-of-ada (as the new provider gave it, unchecked)',
  ]);
});

it('tells no identity under a record that names none', () => {
  const records: AuditRecord[] = [
    {
      id: 'joined',
      action: 'org_admin_joined',
      actor: { kind: 'person', id: 'ada', username: 'ada', displayName: '' },
      subject: null,
      targetId: 'org',
      targetName: null,
      createdAt: 1,
      detail: null,
      identity: null,
    },
  ];
  render(AuditList, { records, locale: 'en' });
  expect(screen.queryByTestId('audit-identity')).toBeNull();
});
