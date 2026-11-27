<script lang="ts">
  import { startPasswordRecovery } from './backend.ts';
  import { forgetFragmentToken } from '$lib/fragment-token.ts';
  import RequestPasswordPage from '$lib/features/account/RequestPasswordPage.svelte';
  import ResetPasswordPage from '$lib/features/account/ResetPasswordPage.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { emailOfForgotPassword } from './recovery-links.ts';
  import type { Router } from './router.svelte.ts';

  const SCOPE = 'reset-password';

  let { router }: { router: Router } = $props();
  const recovery = startPasswordRecovery();
  const isForgot = $derived(router.route.kind === 'passwordForgot');
  const available = $derived(
    isForgot ? recovery.then((client) => client.available()) : Promise.resolve(false),
  );
  let forgotHash = $state(location.hash);
  async function request(email: string): Promise<void> {
    await (await recovery).request(email, getLocale());
  }
  function onhashchange(): void {
    if (isForgot) forgotHash = location.hash;
  }

  async function complete(secret: string, password: string): Promise<void> {
    try {
      await (await recovery).complete(secret, password);
      forgetFragmentToken(SCOPE);
    } catch (error) {
      if (isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token')) {
        forgetFragmentToken(SCOPE);
      }
      throw error;
    }
  }
</script>

<svelte:window {onhashchange} />
{#if isForgot}
  <RequestPasswordPage
    {available}
    {request}
    email={emailOfForgotPassword(forgotHash)}
    homeHref={router.href({})}
  />
{:else}
  {#key router.fragment}
    <ResetPasswordPage token={router.fragment.token} homeHref={router.href({})} {complete} />
  {/key}
{/if}
