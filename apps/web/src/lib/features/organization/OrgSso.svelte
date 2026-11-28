<!--
  One provider per organization; the server proves which session may require it.
  Until one is set up, it is one line ("통합 로그인(SSO): 꺼짐 · 설정") whose
  button opens the form.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { OidcProvider, OidcSettings } from '$lib/state/oidc.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Switch } from '$lib/ui/switch/index.js';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { oidcProblem, pendingOutcome } from './oidc-problem.ts';
  import OrgSsoFormer from './OrgSsoFormer.svelte';
  import OrgSsoPending from './OrgSsoPending.svelte';

  interface Props {
    read: () => Promise<OidcProvider | undefined>;
    save: (settings: OidcSettings) => Promise<OidcProvider>;
    remove: () => Promise<void>;
    onSignIn: () => Promise<void>;
    /** Throws the pending settings away. */
    discard: () => Promise<void>;
    /** Sends the browser to sign in through the pending settings. */
    onTry: () => Promise<void>;
    /** Ends the keeping of the former provider as it was read (`createdAt`), before its time. */
    endFormer: (createdAt: number) => Promise<void>;
  }
  let { read, save, remove, onSignIn, discard, onTry, endFormer }: Props = $props();
  const uid = $props.id();
  let provider = $state.raw<OidcProvider | undefined>(undefined);
  let loaded = $state(false);
  let busy = $state(false);
  let issuer = $state('');
  let clientId = $state('');
  let secret = $state('');
  let replaceSecret = $state(true);
  let autoJoin = $state(true);
  let enabled = $state(true);
  let required = $state(false);
  let problem = $state('');
  let notice = $state('');
  let confirmRemove = $state(false);
  /** The form was asked for while there is no provider. */
  let settingUp = $state(false);
  const folded = $derived(loaded && provider === undefined && !settingUp);
  const enforced = $derived(provider?.ssoRequired ?? false);
  /** Another issuer or client than the stored one: the server asks for its secret anew. */
  const changed = $derived(
    provider !== undefined &&
      (issuer.trim() !== provider.issuer || clientId.trim() !== provider.clientId),
  );
  const askSecret = $derived(replaceSecret || changed);
  /** A secret that may differ from the stored one: proved only by signing in after saving. */
  const secretChanges = $derived(
    askSecret && (secret !== '' || provider?.hasClientSecret === true),
  );
  /** The server's answer for the provider as stored, kept only while nothing that signing in takes changes. */
  const proven = $derived(
    provider?.canRequireSso === true && enabled && !changed && !secretChanges,
  );
  /** Off where it cannot be switched on: what is shown is what is saved. */
  const requires = $derived(required && (enforced || proven));

  function accept(value: OidcProvider | undefined): void {
    provider = value;
    issuer = value?.issuer ?? '';
    clientId = value?.clientId ?? '';
    secret = '';
    replaceSecret = value === undefined;
    autoJoin = value?.autoJoin ?? true;
    enabled = value?.enabled ?? true;
    required = value?.ssoRequired ?? false;
  }

  async function load(): Promise<void> {
    busy = true;
    problem = '';
    try {
      accept(await read());
      loaded = true;
    } catch (error) {
      problem = oidcProblem(error);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    // Back from a sign-in through the pending settings: told once, and taken off the address.
    const outcome = pendingOutcome(new URLSearchParams(location.search));
    if (outcome) history.replaceState(history.state, '', location.pathname);
    void load().then(() => {
      if (outcome?.applied) notice = outcome.words;
      else if (outcome && !problem) problem = outcome.words;
    });
  });

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy) return;
    busy = true;
    problem = '';
    notice = '';
    try {
      const saved = await save({
        issuer: issuer.trim(),
        clientId: clientId.trim(),
        autoJoin,
        enabled,
        ssoRequired: requires,
        ...(askSecret ? { clientSecret: secret } : {}),
      });
      const pendingBefore = provider?.pending?.createdAt;
      accept(saved);
      notice =
        saved.pending && saved.pending.createdAt !== pendingBefore
          ? m.org_sso_pending_saved()
          : m.org_sso_saved();
    } catch (error) {
      problem = oidcProblem(error);
    } finally {
      busy = false;
    }
  }

  async function deleteProvider(): Promise<void> {
    if (busy || enforced) return;
    busy = true;
    problem = '';
    notice = '';
    try {
      await remove();
      accept(undefined);
      settingUp = false;
      notice = m.org_sso_removed();
    } catch (error) {
      problem = oidcProblem(error);
    } finally {
      busy = false;
    }
  }

  async function setUp(): Promise<void> {
    settingUp = true;
    await tick();
    document.getElementById(`${uid}-issuer`)?.focus();
  }

  async function throwAway(): Promise<void> {
    if (busy) return;
    busy = true;
    problem = '';
    notice = '';
    try {
      await discard();
      accept(await read());
      notice = m.org_sso_pending_discarded();
    } catch (error) {
      problem = oidcProblem(error);
    } finally {
      busy = false;
    }
  }

  async function endKeeping(createdAt: number): Promise<void> {
    if (busy) return;
    busy = true;
    problem = '';
    notice = '';
    try {
      await endFormer(createdAt);
      accept(await read());
      notice = m.org_sso_former_ended();
    } catch (error) {
      problem = oidcProblem(error);
      // Refused because another one is kept by now: what is shown is read again.
      await read().then(accept, () => undefined);
    } finally {
      busy = false;
    }
  }

  async function tryPending(): Promise<void> {
    if (busy) return;
    busy = true;
    problem = '';
    try {
      await onTry();
    } catch (error) {
      problem = oidcProblem(error);
      busy = false;
    }
  }

  async function signIn(): Promise<void> {
    if (busy) return;
    busy = true;
    problem = '';
    try {
      await onSignIn();
    } catch (error) {
      problem = oidcProblem(error);
      busy = false;
    }
  }

  async function copy(): Promise<void> {
    if (!provider) return;
    try {
      await navigator.clipboard.writeText(provider.redirectUri);
      notice = m.org_sso_copied();
    } catch {
      problem = m.org_sso_copy_failed();
    }
  }
</script>

<svelte:window
  onpageshow={(event: PageTransitionEvent) => {
    if (event.persisted) busy = false;
  }}
/>

<section class="grid min-w-0 gap-3" aria-labelledby="{uid}-title" data-testid="org-sso">
  {#if folded}
    <div class="flex flex-wrap items-center gap-x-2" data-testid="sso-off">
      <h2 id="{uid}-title" class="text-base font-semibold">{m.org_sso_title()}:</h2>
      <span class="text-muted-foreground">{m.org_sso_off()}</span>
      <span class="text-muted-foreground" aria-hidden="true">·</span>
      <Button
        variant="link"
        class="h-auto px-0 roomy:min-h-touch"
        data-testid="sso-set-up"
        onclick={() => void setUp()}
      >
        {m.org_sso_set_up()}
      </Button>
    </div>
  {:else}
    <h2 id="{uid}-title" class="text-base font-semibold">{m.org_sso_title()}</h2>
    <p class="text-sm text-muted-foreground">{m.org_sso_intro()}</p>
  {/if}
  <p role="alert" class="text-sm text-destructive empty:hidden">{problem}</p>
  <p role="status" class="text-sm text-muted-foreground empty:hidden">{notice}</p>
  {#if !loaded}
    {#if busy}
      <p class="text-sm">{m.app_loading()}</p>
    {:else}
      <Button variant="outline" onclick={() => void load()}>{m.app_retry()}</Button>
    {/if}
  {:else if !folded}
    <form class="grid min-w-0 gap-4 rounded-md border p-4" onsubmit={(event) => void submit(event)}>
      <div class="grid gap-1.5">
        <Label for="{uid}-issuer">{m.org_sso_issuer()}</Label>
        <Input
          id="{uid}-issuer"
          type="url"
          bind:value={issuer}
          required
          disabled={busy}
          autocomplete="off"
          placeholder="https://login.example.com"
          data-testid="sso-issuer"
        />
        <!-- While enforced and kept so, another issuer waits aside and signs nobody out
             yet; switched off in the same save, it is taken at once. -->
        {#if provider && !(enforced && required) && issuer.trim() !== provider.issuer}
          <p class="text-xs text-muted-foreground" data-testid="sso-issuer-change">
            {m.org_sso_issuer_change()}
          </p>
        {/if}
      </div>
      <div class="grid gap-1.5">
        <Label for="{uid}-client">{m.org_sso_client()}</Label>
        <Input
          id="{uid}-client"
          bind:value={clientId}
          required
          disabled={busy}
          autocomplete="off"
          data-testid="sso-client"
        />
      </div>
      <div class="grid gap-1.5">
        {#if askSecret}
          <Label for="{uid}-secret">{m.org_sso_secret()}</Label>
          <Input
            id="{uid}-secret"
            type="password"
            bind:value={secret}
            disabled={busy}
            autocomplete="new-password"
            data-testid="sso-secret"
          />
          <p class="text-xs text-muted-foreground">{m.org_sso_secret_hint()}</p>
          {#if provider?.hasClientSecret && !changed}
            <Button
              variant="outline"
              size="sm"
              class="justify-self-start"
              disabled={busy}
              data-testid="sso-keep-secret"
              onclick={() => {
                replaceSecret = false;
                secret = '';
              }}
            >
              {m.org_sso_keep_secret()}
            </Button>
          {/if}
        {:else}
          <div class="flex flex-wrap items-center justify-between gap-2">
            <span class="text-sm">
              {provider?.hasClientSecret ? m.org_sso_secret_stored() : m.org_sso_secret_none()}
            </span>
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onclick={() => {
                replaceSecret = true;
                secret = '';
              }}
            >
              {m.org_sso_change_secret()}
            </Button>
          </div>
        {/if}
      </div>
      {#if provider}
        <div class="grid min-w-0 gap-1.5">
          <Label for="{uid}-callback">{m.org_sso_callback()}</Label>
          <Input
            id="{uid}-callback"
            readonly
            value={provider.redirectUri}
            data-testid="sso-callback"
          />
          <Button
            class="justify-self-start"
            size="sm"
            variant="outline"
            onclick={() => void copy()}
          >
            {m.org_sso_copy()}
          </Button>
          <p class="text-xs text-muted-foreground">{m.org_sso_callback_hint()}</p>
        </div>
      {:else}
        <p class="text-xs text-muted-foreground">{m.org_sso_callback_after_save()}</p>
      {/if}
      <label class="flex items-start justify-between gap-3">
        <span class="text-sm">{m.org_sso_enabled()}</span>
        <Switch bind:checked={enabled} disabled={busy || enforced} data-testid="sso-enabled" />
      </label>
      <label class="flex items-start justify-between gap-3">
        <span class="text-sm">{m.org_sso_auto_join()}</span>
        <Switch bind:checked={autoJoin} disabled={busy} data-testid="sso-auto-join" />
      </label>
      <div class="grid gap-2 border-t pt-3">
        <label class="flex items-start justify-between gap-3">
          <span class="text-sm">{m.org_sso_required()}</span>
          <Switch
            bind:checked={() => requires, (value: boolean) => (required = value)}
            disabled={busy || (!enforced && !proven)}
            data-testid="sso-required-switch"
          />
        </label>
        <p class="text-xs text-muted-foreground">{m.org_sso_lockout_hint()}</p>
        {#if secretChanges && !enforced}
          <p class="text-sm" data-testid="sso-secret-unproven">{m.org_sso_secret_unproven()}</p>
        {/if}
        {#if !proven && !enforced}
          <p class="text-sm">{m.org_sso_unproven()}</p>
          {#if provider?.enabled}
            <Button
              class="justify-self-start"
              variant="outline"
              disabled={busy}
              data-testid="sso-sign-in"
              onclick={() => void signIn()}
            >
              {m.org_sso_sign_in()}
            </Button>
            <p class="text-xs text-muted-foreground">{m.org_sso_link_hint()}</p>
          {/if}
        {/if}
        {#if enforced}
          <p class="text-sm">{m.org_sso_enforced()}</p>
          <p class="text-xs text-muted-foreground" data-testid="sso-pending-hint">
            {m.org_sso_pending_hint()}
          </p>
        {/if}
      </div>
      {#if provider?.pending}
        <OrgSsoPending
          pending={provider.pending}
          moves={provider.pending.issuer !== provider.issuer}
          throughProvider={enforced || provider.canRequireSso}
          {busy}
          onTry={() => void tryPending()}
          onDiscard={() => void throwAway()}
          onSignInAgain={() => void signIn()}
        />
      {/if}
      {#if provider?.former}
        {@const former = provider.former}
        <OrgSsoFormer {former} {busy} onEnd={() => void endKeeping(former.createdAt)} />
      {/if}
      <div class="flex flex-wrap gap-2">
        <Button
          type="submit"
          disabled={busy || !issuer.trim() || !clientId.trim()}
          data-testid="sso-save"
        >
          {busy ? m.account_working() : m.org_sso_save()}
        </Button>
        {#if provider}
          <Button
            variant="outline"
            disabled={busy || enforced}
            data-testid="sso-delete"
            onclick={() => {
              confirmRemove = true;
            }}
          >
            {m.org_sso_delete()}
          </Button>
        {/if}
      </div>
    </form>
  {/if}
</section>

<ConfirmDialog
  bind:open={confirmRemove}
  title={m.org_sso_delete_title()}
  description={m.org_sso_delete_info()}
  confirmLabel={m.org_sso_delete()}
  destructive
  onConfirm={() => void deleteProvider()}
/>
