<!--
  The provider the organization moved away from, kept for linking only: nobody
  signs in through it, but a member or guest the new provider does not know yet
  signs in through it once to carry their account over. Shown with the moment it ends, and
  a way to end it sooner.
-->
<script lang="ts">
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import type { OidcFormerProvider } from '$lib/state/oidc.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    former: OidcFormerProvider;
    busy: boolean;
    onEnd: () => void;
    /** Omitted in the app: the reader's zone. A fixed zone is useful for examples. */
    timeZone?: string;
  }
  let { former, busy, onEnd, timeZone }: Props = $props();
  const uid = $props.id();
  let confirming = $state(false);
  const until = $derived(
    new Intl.DateTimeFormat(getLocale(), {
      dateStyle: 'long',
      timeStyle: 'short',
      timeZone,
    }).format(former.expiresAt),
  );
</script>

<section
  class="grid min-w-0 gap-2 rounded-md border border-dashed p-4"
  aria-labelledby="{uid}-title"
  data-testid="sso-former"
>
  <h3 id="{uid}-title" class="text-sm font-semibold">{m.org_sso_former_title()}</h3>
  <p class="text-xs text-muted-foreground">{m.org_sso_former_info()}</p>
  <dl class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
    <dt class="text-muted-foreground">{m.org_sso_issuer()}</dt>
    <dd class="break-all" data-testid="sso-former-issuer">{former.issuer}</dd>
    <dt class="text-muted-foreground">{m.org_sso_former_until()}</dt>
    <dd data-testid="sso-former-until">
      <time datetime={new Date(former.expiresAt).toISOString()}>{until}</time>
    </dd>
  </dl>
  <Button
    class="justify-self-start"
    size="sm"
    variant="outline"
    disabled={busy}
    data-testid="sso-former-end"
    onclick={() => {
      confirming = true;
    }}>{m.org_sso_former_end()}</Button
  >
</section>

<ConfirmDialog
  bind:open={confirming}
  title={m.org_sso_former_end_title()}
  description={m.org_sso_former_end_info()}
  confirmLabel={m.org_sso_former_end()}
  destructive
  onConfirm={onEnd}
/>
