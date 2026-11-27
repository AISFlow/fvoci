/**
 * A workspace of an organization that takes no password: what the reader's
 * session is told (401, `sso_required`) is not the session being over, and
 * the workspace is shown closed, with where to sign in. And the requests that
 * connect a single sign-on to the account.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ServerBackend } from '$lib/api/backend.ts';
import { workspacesOf } from '$lib/api/convert.ts';
import { BackendError, isSignedOut, ssoOrgOf, type WorkspaceInfo } from '$lib/state/index.ts';
import { firstChoiceOf } from '$lib/state/workspace-entry.ts';

import { setup, type Setup } from './support.ts';

const ORG = 'the-organization';

const ssoRequired = (): BackendError =>
  new BackendError(401, { reason: 'sso_required', type: '/problems/sso-required', orgId: ORG });

afterEach(() => {
  vi.unstubAllGlobals();
});

/** The world's workspace as the server lists it for an organization, closed or not. */
function listed(made: Setup, closed: boolean): WorkspaceInfo {
  const workspace = { ...made.world.workspace, orgId: ORG, ssoRequired: closed };
  vi.spyOn(made.backend, 'workspaces').mockResolvedValue([workspace]);
  return workspace;
}

describe('a refusal that asks for single sign-on', () => {
  it('names the organization, and is not the session being over', () => {
    expect(ssoOrgOf(ssoRequired())).toBe(ORG);
    expect(isSignedOut(ssoRequired())).toBe(false);
    const over = new BackendError(401, { detail: 'no session token' });
    expect(ssoOrgOf(over)).toBeUndefined();
    expect(isSignedOut(over)).toBe(true);
    expect(isSignedOut(new BackendError(403))).toBe(false);
    expect(ssoOrgOf(new Error('anything else'))).toBeUndefined();
  });

  it('comes from the server with its organization', async () => {
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response(
          JSON.stringify({ status: 401, reason: 'sso_required', orgId: ORG, detail: 'sign in' }),
          { status: 401, headers: { 'Content-Type': 'application/problem+json' } },
        ),
      ),
    );
    const error: unknown = await new ServerBackend('')
      .boards('w')
      .catch((caught: unknown) => caught);
    expect(ssoOrgOf(error)).toBe(ORG);
  });
});

/** A workspace that people share, as `workspacesOf` makes it, open to a password's session. */
const shared = (id: string, title: string, role: 'admin' | 'member', orgId: string) => ({
  id,
  title,
  role,
  orgId,
  kind: 'shared',
  passwordRequired: false,
});

describe('the workspaces as the server lists them', () => {
  it('say which organization each is of, what it is, and which are closed to this session', () => {
    const workspaces = workspacesOf([
      {
        id: 'a',
        orgId: ORG,
        kind: 'shared',
        name: 'A',
        role: 'member',
        ssoRequired: true,
        passwordRequired: false,
      },
      {
        id: 'b',
        orgId: 'another',
        kind: 'shared',
        name: 'B',
        role: 'admin',
        ssoRequired: false,
        passwordRequired: false,
      },
      {
        id: 'c',
        orgId: 'own',
        kind: 'personal',
        name: '',
        role: 'admin',
        ssoRequired: false,
        passwordRequired: false,
      },
    ]);
    expect(workspaces).toEqual([
      { ...shared('a', 'A', 'member', ORG), ssoRequired: true },
      { ...shared('b', 'B', 'admin', 'another'), ssoRequired: false },
      { ...shared('c', '', 'admin', 'own'), kind: 'personal', ssoRequired: false },
    ]);
  });
});

describe('a workspace closed to the session', () => {
  it('is not entered: it is known by its name, and nothing of it is read or followed', async () => {
    const made = setup();
    const closed = listed(made, true);
    const boards = vi.spyOn(made.backend, 'boards');
    // The backend in memory follows nothing; the server's does, by `enter`.
    const follow = vi.fn();
    Object.assign(made.backend, { enter: follow });
    await made.workspace.load();
    expect(made.workspace.status).toBe('ready');
    expect(made.workspace.entered).toBe('sso');
    expect(made.workspace.closed).toEqual(closed);
    expect(made.workspace.info).toBeUndefined();
    expect(made.workspace.containers).toEqual([]);
    expect(boards).not.toHaveBeenCalled();
    expect(follow).not.toHaveBeenCalled();
    // Nothing is asked of a workspace that is not entered.
    expect(await made.workspace.searchBoards('anything')).toEqual([]);
    expect(await made.workspace.createBoard({ title: 'New' })).toBeUndefined();
    expect(boards).not.toHaveBeenCalled();
  });

  it('is not where the reader starts while another of theirs is open', async () => {
    const made = setup();
    const closed = { ...made.world.workspace, id: 'closed', orgId: ORG, ssoRequired: true };
    const open = { ...made.world.workspace, orgId: 'another', ssoRequired: false };
    vi.spyOn(made.backend, 'workspaces').mockResolvedValue([closed, open]);
    await made.workspace.load();
    expect(made.workspace.firstChoice()).toBe(open.id);
    expect(made.workspace.entered).toBe('ready');
    expect(made.workspace.info?.id).toBe(open.id);
  });

  it('is closed when reading it says so, though the list did not', async () => {
    const made = setup();
    const open = listed(made, false);
    vi.spyOn(made.backend, 'boards').mockRejectedValue(ssoRequired());
    await made.workspace.load();
    expect(made.workspace.status).toBe('ready');
    expect(made.workspace.entered).toBe('sso');
    expect(made.workspace.closed).toEqual({ ...open, ssoRequired: true });
    expect(made.workspace.info).toBeUndefined();
  });

  it('closes under the reader when a later request says so, and the session goes on', async () => {
    const made = setup();
    listed(made, false);
    await made.workspace.load();
    expect(made.workspace.entered).toBe('ready');
    expect(made.workspace.containers.length).toBeGreaterThan(0);

    // A refusal of something of another organization, still on its way when the
    // reader came here, closes nothing on the page, and is no failure to show.
    made.workspace.report(
      'save',
      new BackendError(401, { reason: 'sso_required', orgId: 'another-organization' }),
    );
    expect(made.workspace.entered).toBe('ready');
    expect(made.workspace.problems).toEqual([]);

    made.workspace.report('save', ssoRequired());
    expect(made.workspace.status).toBe('ready');
    expect(made.workspace.entered).toBe('sso');
    expect(made.workspace.closed?.orgId).toBe(ORG);
    expect(made.workspace.info).toBeUndefined();
    expect(made.workspace.containers).toEqual([]);
    expect(made.workspace.problems).toEqual([]);
    expect(made.workspace.workspaces.map((one) => one.ssoRequired)).toEqual([true]);

    // A session that is over is still that.
    made.workspace.report('save', new BackendError(401));
    expect(made.workspace.status).toBe('signedOut');
  });
});

describe('connecting a single sign-on to the account', () => {
  interface Sent {
    url: string;
    method: string;
    body: unknown;
  }

  function serve(answer: unknown, status = 200): Sent[] {
    const sent: Sent[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      sent.push({
        url,
        method: init.method ?? 'GET',
        body: typeof init.body === 'string' ? JSON.parse(init.body) : undefined,
      });
      return Promise.resolve(
        new Response(status === 204 ? null : JSON.stringify(answer), {
          status,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    return sent;
  }

  it('carries a held sign-in over, or ends it in a new account, for the organization', async () => {
    const sent = serve({ authorizationUrl: 'https://old.example.com/authorize' });
    const backend = new ServerBackend('');
    expect(await backend.startOidcCarryOver(ORG)).toBe('https://old.example.com/authorize');
    await backend.finishHeldSignIn(ORG);
    expect(sent).toEqual([
      { url: `/api/v1/auth/oidc/held/carry-over?org=${ORG}`, method: 'POST', body: undefined },
      { url: `/api/v1/auth/oidc/held/sign-in?org=${ORG}`, method: 'POST', body: undefined },
    ]);
  });

  it('starts a sign-in with no body, and one that connects with the password', async () => {
    const sent = serve({ authorizationUrl: 'https://idp.example.com/authorize' });
    const backend = new ServerBackend('');
    expect(await backend.startOidc(ORG)).toBe('https://idp.example.com/authorize');
    await backend.startOidc(ORG, 'the password');
    expect(sent).toEqual([
      { url: `/api/v1/auth/oidc/start?org=${ORG}`, method: 'POST', body: undefined },
      {
        url: `/api/v1/auth/oidc/start?org=${ORG}`,
        method: 'POST',
        body: { linkPassword: 'the password' },
      },
    ]);
  });

  it('reads what is connected, and disconnects one with the password', async () => {
    const identity = {
      issuer: 'https://idp.example.com',
      subject: 'there',
      email: 'ada@example.com',
      createdAt: 5,
      madeThisSession: false,
    };
    const read = serve([identity]);
    const backend = new ServerBackend('');
    expect(await backend.identities()).toEqual([identity]);
    expect(read[0]?.url).toBe('/api/v1/users/me/identities');

    const sent = serve(undefined, 204);
    await backend.unlinkIdentity(identity, 'the password');
    expect(sent).toEqual([
      {
        url: '/api/v1/users/me/identities',
        method: 'DELETE',
        body: { issuer: identity.issuer, subject: 'there', password: 'the password' },
      },
    ]);
  });
});

describe('the workspace to start in', () => {
  const one = (id: string, role: 'admin' | 'member' | 'guest', closed: boolean): WorkspaceInfo => ({
    id,
    title: id,
    role,
    orgId: ORG,
    ssoRequired: closed,
  });

  it('is one open to the session that the reader works in, before a guest one', () => {
    expect(
      firstChoiceOf([one('guest', 'guest', false), one('works', 'member', false)], undefined),
    ).toBe('works');
  });

  it('is an open one the reader is a guest of before a closed one they work in', () => {
    expect(
      firstChoiceOf([one('closed', 'member', true), one('guest', 'guest', false)], undefined),
    ).toBe('guest');
  });

  it('is the first when all are closed, and the last one when it is still theirs', () => {
    const all = [one('a', 'guest', true), one('b', 'member', true)];
    expect(firstChoiceOf(all, undefined)).toBe('a');
    expect(firstChoiceOf(all, 'b')).toBe('b');
  });

  describe('beside the reader’s own space, listed last', () => {
    const own: WorkspaceInfo = { id: 'own', title: '', kind: 'personal', role: 'admin' };
    const shared = (
      id: string,
      role: 'admin' | 'member' | 'guest',
      closed = false,
    ): WorkspaceInfo => ({
      ...one(id, role, closed),
      kind: 'shared',
    });

    it('is the workspace of the organization a new account started, not its own space', () => {
      expect(firstChoiceOf([shared('made', 'admin'), own], undefined)).toBe('made');
    });

    it('is the workspace an account is only a guest of, not its own space', () => {
      expect(firstChoiceOf([shared('invited', 'guest'), own], undefined)).toBe('invited');
    });

    it('is a shared workspace closed to the session before the own space', () => {
      expect(firstChoiceOf([shared('closed', 'member', true), own], undefined)).toBe('closed');
    });

    it('is the own space of an account in no organization, and the own space it was last in', () => {
      expect(firstChoiceOf([own], undefined)).toBe('own');
      expect(firstChoiceOf([shared('made', 'admin'), own], 'own')).toBe('own');
    });

    it('is a shared workspace also where the list puts the own space first', () => {
      expect(firstChoiceOf([own, shared('made', 'member')], undefined)).toBe('made');
    });
  });
});
