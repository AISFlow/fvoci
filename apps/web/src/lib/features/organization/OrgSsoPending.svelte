<!--
  The provider's pending settings: saved aside while single sign-on is required,
  and used by nobody until a sign-in through them succeeds, which applies them.
  The secret is never shown, only whether one is saved.

  Settings of another issuer move the organization to another provider, where
  the administrator's identity is usually connected to no account yet. The
  sign-in through them connects it to the administrator's own account only from
  a session made through the current provider no more than five minutes before
  the sign-in comes back: the section says until when that holds, or offers to
  sign in again first, which comes back to this page.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import type { OidcPendingProvider } from '$lib/state/oidc.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    pending: OidcPendingProvider;
    /** Whether the pending settings are of another issuer than the provider's. */
    moves: boolean;
    /**
     * Whether the reader's session came through this provider (as it is now or as
     * it was): signing in again then leads back into the same account.
     */
    throughProvider: boolean;
    busy: boolean;
    onTry: () => void;
    onDiscard: () => void;
    /** Sends the browser to sign in through the current provider, to come back here. */
    onSignInAgain: () => void;
    /** Omitted in the app: the reader's zone. A fixed zone is useful for examples. */
    timeZone?: string;
  }
  let { pending, moves, throughProvider, busy, onTry, onDiscard, onSignInAgain, timeZone }: Props =
    $props();
  const uid = $props.id();
  /**
   * Until when a sign-in through them from this session connects a new identity, as
   * the server judged by its own clock when it was read; undefined when it does not.
   */
  const bindsUntil = $derived(pending.bindsIdentityUntil ?? undefined);
  const until = $derived(
    bindsUntil === undefined
      ? ''
      : new Intl.DateTimeFormat(getLocale(), { timeStyle: 'short', timeZone }).format(bindsUntil),
  );
  /** Signing in again comes first where the trial would not connect the new identity. */
  const signInFirst = $derived(moves && bindsUntil === undefined);
</script>

<section
  class="grid min-w-0 gap-2 rounded-md border border-dashed p-4"
  aria-labelledby="{uid}-title"
  data-testid="sso-pending"
>
  <h3 id="{uid}-title" class="text-sm font-semibold">{m.org_sso_pending_title()}</h3>
  <p class="text-xs text-muted-foreground">{m.org_sso_pending_info()}</p>
  <dl class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
    <dt class="text-muted-foreground">{m.org_sso_issuer()}</dt>
    <dd class="break-all" data-testid="sso-pending-issuer">{pending.issuer}</dd>
    <dt class="text-muted-foreground">{m.org_sso_client()}</dt>
    <dd class="break-all" data-testid="sso-pending-client">{pending.clientId}</dd>
    <dt class="text-muted-foreground">{m.org_sso_secret()}</dt>
    <dd>{pending.hasClientSecret ? m.org_sso_secret_stored() : m.org_sso_secret_none()}</dd>
  </dl>
  {#if moves}
    <p class="text-sm" data-testid="sso-pending-binding">
      {signInFirst ? m.org_sso_pending_bind_stale() : m.org_sso_pending_bind_fresh({ until })}
    </p>
  {/if}
  <div class="flex flex-wrap gap-2">
    {#if signInFirst && throughProvider}
      <Button
        size="sm"
        disabled={busy}
        data-testid="sso-pending-sign-in-again"
        onclick={onSignInAgain}
      >
        {m.org_sso_pending_sign_in_again()}
      </Button>
    {/if}
    <Button
      size="sm"
      variant={signInFirst && throughProvider ? 'outline' : 'default'}
      disabled={busy}
      data-testid="sso-pending-try"
      onclick={onTry}
    >
      {m.org_sso_pending_try()}
    </Button>
    <Button
      size="sm"
      variant="outline"
      disabled={busy}
      data-testid="sso-pending-discard"
      onclick={onDiscard}
    >
      {m.org_sso_pending_discard()}
    </Button>
  </div>
</section>
