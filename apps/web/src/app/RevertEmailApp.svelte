<script lang="ts">
  import { startEmailChange } from './backend.ts';
  import { forgetFragmentToken } from '$lib/fragment-token.ts';
  import RevertEmailPage from '$lib/features/account/RevertEmailPage.svelte';
  import { isBackendError } from '$lib/state/index.ts';
  import { forgotPasswordHref } from './recovery-links.ts';
  import type { Router } from './router.svelte.ts';

  const SCOPE = 'revert-email';

  let { router }: { router: Router } = $props();
  const change = startEmailChange();

  /** Gives the address back by the token; it then leaves `sessionStorage` too. */
  async function revert(token: string): Promise<string | undefined> {
    try {
      const email = await (await change).revert(token);
      forgetFragmentToken(SCOPE);
      return email;
    } catch (error) {
      if (isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token')) {
        forgetFragmentToken(SCOPE);
      }
      throw error;
    }
  }

  /** The page that asks for a reset link, filled in with the address given back. */
  function resetHref(email: string | undefined): string {
    return forgotPasswordHref(router.href({}), email);
  }
</script>

{#key router.fragment}
  <RevertEmailPage token={router.fragment.token} {revert} homeHref={router.href({})} {resetHref} />
{/key}
