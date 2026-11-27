<script lang="ts">
  import { onMount, untrack } from 'svelte';

  import { acceptedRoute } from './accepted-route.ts';
  import { startEmailVerification } from './backend.ts';
  import { forgetFragmentToken } from '$lib/fragment-token.ts';
  import VerifyEmailPage from '$lib/features/account/VerifyEmailPage.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import type { Confirmed } from '$lib/state/email-verification.ts';
  import { isBackendError } from '$lib/state/index.ts';
  import type { Router } from './router.svelte.ts';

  const SCOPE = 'verify-email';

  let { router }: { router: Router } = $props();
  const verification = startEmailVerification();

  /**
   * Confirms, and goes to where the account was let in: the welcome board of an
   * organization it started, the board or the wiki of a guest's link, or the
   * workspace. The token leaves the address and the history on entry, and
   * `sessionStorage` once settled.
   */
  async function complete(token: string, password?: string): Promise<Confirmed> {
    const client = await verification;
    let confirmed: Confirmed;
    try {
      confirmed = await client.complete(token, password);
    } catch (error) {
      if (isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token')) {
        forgetFragmentToken(SCOPE);
      }
      throw error;
    }
    forgetFragmentToken(SCOPE);
    let route = acceptedRoute(confirmed);
    if (confirmed.organizationStarted) {
      // The account is in: no welcome board is no reason to keep it out.
      const boardId = await client.welcome(confirmed.workspaceId).catch(() => undefined);
      route = boardId
        ? { kind: 'board', workspaceId: confirmed.workspaceId, boardId }
        : { kind: 'workspace', workspaceId: confirmed.workspaceId };
    }
    location.replace(router.href(route));
    return confirmed;
  }

  /** Sends the confirmation of the token, if there is one: once for each. */
  function confirmToken(token: string): Promise<Confirmed> | undefined {
    return token ? complete(token) : undefined;
  }

  const initialToken = untrack(() => router.fragment.token);
  let currentToken = $state(initialToken);
  let confirming = $state.raw(confirmToken(initialToken));

  onMount(() =>
    router.onFragment(({ token }) => {
      currentToken = token;
      confirming = confirmToken(token);
    }),
  );

  /** Confirms again by the kept token, with the registration's password. */
  function onPassword(password: string): void {
    confirming = currentToken ? complete(currentToken, password) : undefined;
  }

  async function resend(email: string): Promise<void> {
    await (await verification).resend({ email }, getLocale());
  }
</script>

<VerifyEmailPage {confirming} homeHref={router.href({})} {resend} {onPassword} />
