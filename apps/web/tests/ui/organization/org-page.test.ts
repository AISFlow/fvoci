/**
 * Removing a member on the organization screen: hidden on the administrator's
 * own row, confirmed before sending, shows 409 refusal reasons clearly, and
 * refreshes the list when the member is already gone (404). The policies: the
 * switch of API tokens only while single sign-on is required, and a name being
 * typed that stays through the other changes.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrgPage from '$lib/features/organization/OrgPage.svelte';
import { BackendError } from '$lib/state/index.ts';
import { OrgAdmin } from '$lib/state/org-admin.svelte.ts';
import type { OidcProvider } from '$lib/state/oidc.ts';
import type { Organization, Organizations, OrgMember } from '$lib/state/organization.ts';

afterEach(cleanup);

const ADA: OrgMember = { userId: 'u1', username: 'ada', displayName: 'Ada', role: 'admin' };
const BEA: OrgMember = { userId: 'u2', username: 'bea', displayName: 'Bea', role: 'member' };

function apiWith(
  members: OrgMember[],
  removeMember: Organizations['removeMember'] = vi.fn(() => Promise.resolve()),
): { api: Organizations; removeMember: Organizations['removeMember'] } {
  const api = {
    organization: () =>
      Promise.resolve({ id: 'o1', name: 'Acme', publicLinks: true, defaultWorkspaceId: 'w1' }),
    workspaces: () => Promise.resolve([]),
    members: () => Promise.resolve([...members]),
    records: () => Promise.resolve([]),
    oidcProvider: () => Promise.resolve(undefined),
    removeMember,
  } as unknown as Organizations;
  return { api, removeMember };
}

async function mount(members: OrgMember[], removeMember?: Organizations['removeMember']) {
  const made = apiWith(members, removeMember);
  const admin = new OrgAdmin(made.api, 'o1');
  await admin.load();
  render(OrgPage, {
    props: {
      admin,
      locale: 'en-US',
      meId: 'u1',
      homeHref: '/',
      workspaceHref: (id: string) => `/w/${id}`,
      onSsoSignIn: () => Promise.resolve(),
    },
  });
  return { admin, removeMember: made.removeMember };
}

describe('removing a member from the organization screen', () => {
  it('hides Remove on the reader’s own row and confirms before removing another member', async () => {
    const members = [ADA, BEA];
    const removeMember = vi.fn<Organizations['removeMember']>(() => {
      members.splice(1, 1);
      return Promise.resolve();
    });
    await mount(members, removeMember);

    const list = screen.getByTestId('org-members');
    const ownRow = list.querySelector<HTMLElement>('[data-username="ada"]');
    const otherRow = list.querySelector<HTMLElement>('[data-username="bea"]');
    if (!ownRow || !otherRow) throw new Error('expected member rows');
    expect(within(ownRow).queryByRole('button', { name: /Remove/ })).toBeNull();

    await fireEvent.click(within(otherRow).getByRole('button', { name: /Remove Bea/ }));
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog).toHaveTextContent('Remove Bea (@bea) from the organization?');
    expect(removeMember).not.toHaveBeenCalled();

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Remove' }));
    expect(removeMember).toHaveBeenCalledExactlyOnceWith('o1', 'u2');
    await waitFor(() => {
      expect(screen.getByTestId('org-members')).not.toHaveTextContent('@bea');
    });
  });

  it('shows a clear refusal message on 409 and reloads the member list on 404', async () => {
    const members = [ADA, BEA];
    const removeMember = vi
      .fn<Organizations['removeMember']>()
      .mockRejectedValueOnce(new BackendError(409, { reason: 'sole_org_admin' }))
      .mockImplementationOnce(() => {
        members.splice(1, 1);
        return Promise.reject(new BackendError(404));
      });
    await mount(members, removeMember);

    const list = screen.getByTestId('org-members');
    await fireEvent.click(within(list).getByRole('button', { name: /Remove Bea/ }));
    await fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Remove' }),
    );
    expect(await screen.findByTestId('org-refusal')).toHaveTextContent(
      'This organization must keep an administrator.',
    );

    await fireEvent.click(within(list).getByRole('button', { name: /Remove Bea/ }));
    await fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Remove' }),
    );
    expect(await screen.findByTestId('org-refusal')).toHaveTextContent('It is not there any more.');
    await waitFor(() => {
      expect(screen.getByTestId('org-members')).not.toHaveTextContent('@bea');
    });
  });

  it('hides Remove buttons when meId is unknown', async () => {
    const admin = new OrgAdmin(apiWith([ADA, BEA]).api, 'o1');
    await admin.load();
    render(OrgPage, {
      props: {
        admin,
        locale: 'en-US',
        meId: undefined,
        homeHref: '/',
        workspaceHref: (id: string) => `/w/${id}`,
        onSsoSignIn: () => Promise.resolve(),
      },
    });
    const list = screen.getByTestId('org-members');
    expect(within(list).queryByRole('button', { name: /Remove/ })).toBeNull();
  });
});

/** An organization `Acme` whose provider is `provider`, changed in place by `changeOrganization`. */
function policyApi(provider: OidcProvider | undefined): {
  api: Organizations;
  changeOrganization: ReturnType<typeof vi.fn<Organizations['changeOrganization']>>;
} {
  let organization: Organization = {
    id: 'o1',
    name: 'Acme',
    publicLinks: true,
    allowCopyOut: true,
    allowApiTokens: false,
    defaultWorkspaceId: 'w1',
  };
  const changeOrganization = vi.fn<Organizations['changeOrganization']>((_, patch) => {
    if (patch.name === 'Taken') {
      return Promise.reject(new BackendError(409, { reason: 'workspace_name_taken' }));
    }
    organization = { ...organization, ...patch };
    return Promise.resolve(organization);
  });
  const api = {
    organization: () => Promise.resolve({ ...organization }),
    workspaces: () => Promise.resolve([]),
    members: () => Promise.resolve([ADA]),
    records: () => Promise.resolve([]),
    oidcProvider: () => Promise.resolve(provider),
    apiTokens: { list: () => Promise.resolve([]) },
    changeOrganization,
  } as unknown as Organizations;
  return { api, changeOrganization };
}

const PROVIDER: OidcProvider = {
  issuer: 'https://id.example',
  clientId: 'fvoci',
  hasClientSecret: true,
  autoJoin: true,
  enabled: true,
  ssoRequired: false,
  canRequireSso: false,
  redirectUri: 'https://fvoci.example/oidc',
  createdAt: 1,
  updatedAt: 1,
  pending: null,
  former: null,
};

async function mountPolicy(
  provider: OidcProvider | undefined,
  renamedOrganization?: (orgId: string, name: string) => void,
) {
  const made = policyApi(provider);
  const admin = new OrgAdmin(made.api, 'o1', { report: vi.fn(), renamedOrganization });
  await admin.load();
  render(OrgPage, {
    props: {
      admin,
      locale: 'en-US',
      meId: 'u1',
      homeHref: '/',
      workspaceHref: (id: string) => `/w/${id}`,
      onSsoSignIn: () => Promise.resolve(),
    },
  });
  await screen.findByTestId('org-policy');
  return { admin, ...made };
}

describe('the policies of the organization', () => {
  it('shows no switch of API tokens where single sign-on is not set up, or not required', async () => {
    const { admin } = await mountPolicy(undefined);
    await screen.findByTestId('sso-set-up');
    expect(admin.ssoRequired).toBe(false);
    expect(screen.queryByTestId('org-allow-api-tokens')).toBeNull();
    cleanup();

    await mountPolicy(PROVIDER);
    await screen.findByLabelText(/issuer/i);
    expect(screen.queryByTestId('org-allow-api-tokens')).toBeNull();
  });

  it('shows the switch of API tokens while the organization takes its single sign-on alone', async () => {
    const { admin } = await mountPolicy({ ...PROVIDER, ssoRequired: true, canRequireSso: true });
    expect(await screen.findByTestId('org-allow-api-tokens')).not.toBeChecked();
    expect(admin.ssoRequired).toBe(true);
  });

  it('keeps a name being typed through a switch, and through its refusal', async () => {
    const { changeOrganization } = await mountPolicy(undefined);
    const field = screen.getByTestId('org-name');
    await fireEvent.input(field, { target: { value: 'Taken' } });
    await fireEvent.click(screen.getByTestId('org-public-links'));
    await waitFor(() => {
      expect(changeOrganization).toHaveBeenCalledWith('o1', { publicLinks: false });
    });
    await waitFor(() => {
      expect(screen.getByTestId('org-public-links')).not.toBeChecked();
    });
    expect(screen.getByTestId('org-name')).toHaveValue('Taken');

    await waitFor(() => {
      expect(screen.getByTestId('org-name-save')).toBeEnabled();
    });
    await fireEvent.click(screen.getByTestId('org-name-save'));
    expect(await screen.findByTestId('org-refusal')).toBeInTheDocument();
    expect(screen.getByTestId('org-name')).toHaveValue('Taken');
    expect(screen.getByTestId('org-name')).toHaveAttribute('autocomplete', 'off');
  });

  it('tells the app the name the organization was given', async () => {
    const renamed = vi.fn<(orgId: string, name: string) => void>();
    await mountPolicy(undefined, renamed);
    await fireEvent.input(screen.getByTestId('org-name'), { target: { value: '  Acme Labs ' } });
    await fireEvent.click(screen.getByTestId('org-name-save'));
    await waitFor(() => {
      expect(renamed).toHaveBeenCalledExactlyOnceWith('o1', 'Acme Labs');
    });
    expect(screen.getByTestId('org-name')).toHaveValue('Acme Labs');
  });
});
