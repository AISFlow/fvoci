<!--
  Deleting the reader's account, in the settings. Folded until asked for: it
  then reads what the deletion comes to (`getAccountDeletion`) and says it
  before anything is asked: the days the account waits, that what was written
  stays under "Deleted user", the reader's own space, which always goes with
  the account, and apart from it the organizations that have nobody else,
  which go too, with how much is in each. Who is the only
  administrator of the server, or of an organization with other people in it,
  is told that and gets no form.

  The request proves that the account is the reader's: its name typed out,
  and its password, or for an account without one a sign-in through the
  provider made just before. Asked, every session of the account is over:
  the page starts again (`onDelete` resolves when it is about to).
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError, type AccountDeletion } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import {
    deletionProblemOf,
    holdingsOf,
    leavingOf,
    namesAccount,
    wrongName,
    type DeletionProblem,
  } from './account-deletion.ts';

  interface Props {
    /** The account's login name, which the reader types to say they mean it. */
    username: string;
    /** What deleting the account comes to, as things are now. */
    read: () => Promise<AccountDeletion>;
    /** Asks for the deletion; a refusal rejects. */
    onDelete: (username: string, password: string) => Promise<void>;
  }

  let { username, read, onDelete }: Props = $props();

  const uid = $props.id();
  let open = $state(false);
  /** What the deletion comes to; undefined until it is read, and when it could not be. */
  let standing = $state.raw<AccountDeletion | undefined>(undefined);
  let unread = $state(false);
  let typed = $state('');
  let password = $state('');
  let working = $state(false);
  let problem = $state.raw<DeletionProblem | undefined>(undefined);
  let toggle = $state<HTMLElement | null>(null);
  let typedField = $state<HTMLInputElement | null>(null);
  let passwordField = $state<HTMLInputElement | null>(null);
  let submitButton = $state<HTMLElement | null>(null);
  /** The readings begun: one that ends after the section was folded, or read again, is let go. */
  let readings = 0;

  /** Somebody else is made an administrator first: there is nothing to ask yet. */
  const held = $derived(
    standing !== undefined && (standing.lastInstanceAdmin || standing.soleAdminOf.length > 0),
  );

  function fold(): void {
    readings += 1;
    open = false;
    standing = undefined;
    unread = false;
    typed = '';
    password = '';
    problem = undefined;
  }

  async function show(): Promise<void> {
    const reading = (readings += 1);
    open = true;
    unread = false;
    try {
      const answered = await read();
      if (reading !== readings) return;
      standing = answered;
      await tick();
      typedField?.focus();
    } catch {
      if (reading !== readings) return;
      standing = undefined;
      unread = true;
    }
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    if (!namesAccount(typed, username)) {
      problem = wrongName();
      typedField?.focus();
      return;
    }
    working = true;
    problem = undefined;
    try {
      // The page starts again signed out: the form stays as it is until then.
      await onDelete(typed.trim(), password);
    } catch (error) {
      problem = deletionProblemOf(error);
      working = false;
      // The only administrator of something since it was read: read again, which says of what.
      if (isBackendError(error) && error.status === 409) {
        standing = await read().catch(() => standing);
      }
      // The button was out of use while it worked, and lost the focus with that.
      await tick();
      if (problem.field === 'username') typedField?.focus();
      else if (problem.field === 'password') passwordField?.focus();
      else submitButton?.focus();
    }
  }
</script>

<div class="grid gap-3 border-t pt-4" data-testid="account-deletion">
  <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
    <span class="min-w-0 flex-1 font-medium">{m.account_delete_title()}</span>
    <Button
      bind:ref={toggle}
      variant="outline"
      class="roomy:min-h-touch"
      aria-expanded={open}
      aria-controls={open ? `${uid}-body` : undefined}
      disabled={working}
      onclick={() => {
        if (open) {
          fold();
          toggle?.focus();
        } else void show();
      }}
    >
      {open ? m.shared_cancel() : m.account_delete_title()}
    </Button>
  </div>

  {#if open}
    <div id="{uid}-body" class="grid gap-3">
      {#if unread}
        <p class="text-sm text-destructive" role="alert">{m.account_delete_unread()}</p>
        <Button
          variant="outline"
          class="justify-self-start roomy:min-h-touch"
          onclick={() => void show()}
        >
          {m.app_retry()}
        </Button>
      {:else if !standing}
        <p class="text-sm text-muted-foreground" role="status">{m.account_working()}</p>
      {:else if held}
        <div class="grid gap-2 text-sm" data-testid="account-deletion-held">
          {#if standing.lastInstanceAdmin}
            <p>{m.account_delete_last_instance_admin()}</p>
          {/if}
          {#if standing.soleAdminOf.length > 0}
            <p>{m.account_delete_sole_admin()}</p>
            <ul class="list-disc ps-5">
              {#each standing.soleAdminOf as organization (organization.id)}
                <li>{organization.name || m.account_delete_organization_unnamed()}</li>
              {/each}
            </ul>
          {/if}
        </div>
      {:else}
        <p class="text-sm text-muted-foreground" data-testid="account-deletion-text">
          {m.account_delete_text({ days: standing.graceDays })}
        </p>
        {@const leaving = leavingOf(standing.organizations)}
        {#if leaving.own}
          <p class="text-sm font-medium text-destructive" data-testid="account-deletion-own-space">
            {leaving.own.boards === undefined
              ? m.account_delete_own_space_plain()
              : m.account_delete_own_space(holdingsOf(leaving.own))}
          </p>
        {/if}
        {#if leaving.alone.length > 0}
          <div class="grid gap-2 text-sm" data-testid="account-deletion-organizations">
            <p class="font-medium text-destructive">{m.account_delete_organizations()}</p>
            <ul class="list-disc ps-5">
              {#each leaving.alone as organization (organization.id)}
                {@const name = organization.name || m.account_delete_organization_unnamed()}
                <li>
                  {organization.boards === undefined
                    ? m.account_delete_organization_closed({ name })
                    : m.account_delete_organization_counts({ name, ...holdingsOf(organization) })}
                </li>
              {/each}
            </ul>
          </div>
        {/if}
        <form class="grid gap-3" onsubmit={(event) => void submit(event)}>
          <div class="grid gap-1.5">
            <Label for="{uid}-name">{m.account_delete_confirm_name({ username })}</Label>
            <Input
              bind:ref={typedField}
              id="{uid}-name"
              bind:value={typed}
              autocomplete="off"
              autocapitalize="none"
              spellcheck={false}
              required
              aria-invalid={problem?.field === 'username' ? 'true' : undefined}
              aria-describedby={problem?.field === 'username' ? `${uid}-problem` : undefined}
              class="roomy:h-touch roomy:text-base"
            />
          </div>
          {#if standing.proof === 'password'}
            <div class="grid gap-1.5">
              <Label for="{uid}-password">{m.account_password_current()}</Label>
              <Input
                bind:ref={passwordField}
                id="{uid}-password"
                type="password"
                bind:value={password}
                autocomplete="current-password"
                required
                aria-invalid={problem?.field === 'password' ? 'true' : undefined}
                aria-describedby={problem?.field === 'password' ? `${uid}-problem` : undefined}
                class="roomy:h-touch roomy:text-base"
              />
            </div>
          {:else}
            <p class="text-sm text-muted-foreground">{m.account_delete_sign_in_first()}</p>
          {/if}
          {#if problem}
            <p id="{uid}-problem" class="text-sm text-destructive" role="alert">{problem.text}</p>
          {/if}
          <Button
            bind:ref={submitButton}
            type="submit"
            variant="destructive"
            class="justify-self-start roomy:min-h-touch"
            disabled={working}
          >
            {working ? m.account_working() : m.account_delete_submit()}
          </Button>
        </form>
      {/if}
    </div>
  {/if}
</div>
