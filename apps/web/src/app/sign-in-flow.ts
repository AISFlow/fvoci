/**
 * Post-authentication workspace load flow for `SignInPage` in `App.svelte`,
 * extracted so `App.svelte` stays concise and under 500 lines. Welcome boards
 * are created idempotently on the server (`workspaces.welcomed_at`).
 */
import {
  BackendError,
  type Account,
  type AccountSettings,
  type Workspace,
} from '$lib/state/index.ts';

import { workspaceIdOf, type Router } from './router.svelte.ts';

export interface SignInFlowContext {
  account: Account | undefined;
  workspace: Workspace | undefined;
  router: Router;
  registered: { username: string; email: string } | undefined;
  clearRegistered(): void;
  leaveAccount(): void;
  followsLanguage(settings: AccountSettings): boolean;
}

export async function completePasswordSignIn(
  ctx: SignInFlowContext,
  login: string,
  password: string,
): Promise<void> {
  await ctx.account?.signIn(login, password);
  const { workspace, router } = ctx;
  const before = workspace?.me;
  if (workspace && before) {
    const now = await workspace.backend.me().catch(() => undefined);
    if (now?.id === before.id) location.reload();
    else {
      ctx.leaveAccount();
      router.restart({});
    }
    return;
  }
  if (router.route.kind === 'unknown') router.go({}, { replace: true });
  const isNew = login === ctx.registered?.username || login === ctx.registered?.email;
  await workspace?.load(
    workspaceIdOf(router.route),
    isNew ? undefined : (settings) => ctx.followsLanguage(settings),
  );
  if (workspace?.status === 'signedOut') {
    throw new BackendError(401, {
      reason: 'session_not_kept',
      detail: 'the session was not kept',
    });
  }
  if (ctx.registered && workspace?.status === 'ready') {
    ctx.clearRegistered();
  }
  if (workspace?.status === 'ready') ctx.followsLanguage(workspace.settings);
}
