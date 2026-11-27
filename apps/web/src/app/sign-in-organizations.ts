/**
 * The reader's organizations that sign in through a provider: what the
 * connected sign-ins of the settings offer to connect, each once, whichever
 * of its workspaces the reader is in. One whose provider is off, or that has
 * none, is left out.
 */
import {
  isOwnSpace,
  type Account,
  type LoginOptions,
  type SignInOrganization,
  type WorkspaceInfo,
} from '$lib/state/index.ts';

export async function signInOrganizations(
  workspaces: readonly WorkspaceInfo[],
  loginOptions: (orgId: string) => Promise<LoginOptions>,
): Promise<SignInOrganization[]> {
  // An organization once, by the workspaces of it the reader is in: those are the
  // names they know it by. Its own name stands in where none of them has one.
  const titles = new Map<string, string[]>();
  for (const workspace of workspaces) {
    // The reader's own space has no sign-in of its own.
    if (workspace.orgId === undefined || isOwnSpace(workspace)) continue;
    const known = titles.get(workspace.orgId) ?? [];
    titles.set(workspace.orgId, workspace.title ? [...known, workspace.title] : known);
  }
  const found = await Promise.all(
    [...titles].map(async ([orgId, known]) => {
      // One that cannot be read is not offered; the others still are.
      const options = await loginOptions(orgId).catch(() => undefined);
      const organization = options?.organization;
      if (!organization?.oidc) return undefined;
      return { orgId, name: known.join(', ') || organization.name || orgId };
    }),
  );
  return found.filter((one) => one !== undefined);
}

/**
 * What the sign-in page does through the account of the backend: what it reads
 * of the server, the start of a sign-in through an organization's provider
 * (`toProvider`, which sends the browser there), and the two ends of a sign-in
 * that is held while the organization moves to another provider. `home` is the
 * address of the app, where the browser goes on signed in.
 */
export function signInWays(
  account: Account | undefined,
  toProvider: (orgId: string) => Promise<void>,
  home: string,
): {
  optionsOf: (orgId: string | undefined) => Promise<LoginOptions>;
  onOidcStart: (orgId: string) => Promise<void>;
  onCarryOver: (orgId: string) => Promise<void>;
  onStartAnew: (orgId: string) => Promise<void>;
} {
  return {
    optionsOf: (orgId) =>
      account?.loginOptions(orgId) ?? Promise.resolve({ registrationOpen: true }),
    onOidcStart: toProvider,
    onCarryOver: async (orgId) => {
      if (!account) throw new Error('no backend to sign in to');
      location.assign(await account.startOidcCarryOver(orgId));
    },
    onStartAnew: async (orgId) => {
      if (!account) throw new Error('no backend to sign in to');
      await account.finishHeldSignIn(orgId);
      // The session came with the answer: the app loads again, signed in.
      location.assign(home);
    },
  };
}
