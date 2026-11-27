<!--
  What signing in to an account that waits to be deleted shows: that it is to
  be deleted, in how many days and on which day, what goes with it, what holds
  it back if it became the only administrator of something meanwhile, and the
  one thing its session opens, taking the deletion back. Taken back, the
  account is as it was, and the page starts again in it. Signing out leaves
  the account waiting.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { AccountDeletion } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { daysLeft, leavingOf, waitsNoMore } from './account-deletion.ts';

  interface Props {
    /** Until when the account waits, and what goes with it. */
    read: () => Promise<AccountDeletion>;
    /** Takes the deletion back; a refusal rejects. */
    onCancel: () => Promise<void>;
    /**
     * The account does not wait any more (the deletion was taken back
     * elsewhere, or the account is erased): the page reads again who is here.
     */
    onStale: () => void;
    onSignOut: () => void;
    /** The language the day is written in. */
    locale: string;
  }

  let { read, onCancel, onStale, onSignOut, locale }: Props = $props();

  /** Until when it waits; undefined until it is read, and when it could not be. */
  let standing = $state.raw<AccountDeletion | undefined>(undefined);
  let unread = $state(false);
  let working = $state(false);
  let failed = $state(false);
  let cancelButton = $state<HTMLElement | null>(null);

  const deleteAfter = $derived(standing?.deleteAfter);
  const days = $derived(deleteAfter === undefined ? 0 : daysLeft(deleteAfter, Date.now()));
  const day = $derived(
    deleteAfter === undefined
      ? ''
      : new Intl.DateTimeFormat(locale, { dateStyle: 'long' }).format(deleteAfter),
  );

  async function load(): Promise<void> {
    unread = false;
    try {
      const answered = await read();
      // No day to be deleted on: it was taken back while this was read.
      if (answered.deleteAfter === undefined) onStale();
      else standing = answered;
    } catch (error) {
      if (waitsNoMore(error)) {
        onStale();
        return;
      }
      standing = undefined;
      unread = true;
    }
  }
  onMount(() => {
    void load();
  });

  async function cancel(): Promise<void> {
    if (working) return;
    working = true;
    failed = false;
    try {
      // The page starts again in the account: the button stays as it is until then.
      await onCancel();
    } catch (error) {
      if (waitsNoMore(error)) {
        onStale();
        return;
      }
      failed = true;
      working = false;
      // The button was out of use while it worked, and lost the focus with that.
      await tick();
      cancelButton?.focus();
    }
  }
</script>

<main class="grid min-h-dvh place-items-center p-4" data-testid="account-leaving">
  <div class="grid max-w-md justify-items-center gap-3 text-center">
    <h1 class="text-lg font-medium">{m.account_leaving_title()}</h1>
    {#if unread}
      <p class="text-destructive" role="alert">{m.account_delete_unread()}</p>
      <Button variant="outline" class="roomy:min-h-touch" onclick={() => void load()}>
        {m.app_retry()}
      </Button>
    {:else if !standing}
      <p class="text-muted-foreground" role="status">{m.account_working()}</p>
    {:else}
      <p class="text-muted-foreground" data-testid="account-leaving-text">
        {days > 1
          ? m.account_leaving_text({ days, day })
          : days === 1
            ? m.account_leaving_text_one({ day })
            : m.account_leaving_due()}
      </p>
      {#if standing.lastInstanceAdmin || standing.soleAdminOf.length > 0}
        <!-- Since it asked, the account became the only administrator of something: it is not erased. -->
        <div class="grid gap-1 text-sm" data-testid="account-leaving-held">
          <p class="text-destructive">{m.account_leaving_held()}</p>
          <ul>
            {#if standing.lastInstanceAdmin}
              <li class="font-medium">{m.account_leaving_held_server()}</li>
            {/if}
            {#each standing.soleAdminOf as organization (organization.id)}
              <li class="font-medium">
                {organization.name || m.account_delete_organization_unnamed()}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
      {@const leaving = leavingOf(standing.organizations)}
      {#if leaving.own}
        <p class="text-sm" data-testid="account-leaving-own-space">
          {m.account_leaving_own_space()}
        </p>
      {/if}
      {#if leaving.alone.length > 0}
        <div class="grid gap-1 text-sm" data-testid="account-leaving-organizations">
          <p>{m.account_leaving_organizations()}</p>
          <ul>
            {#each leaving.alone as organization (organization.id)}
              <li class="font-medium">
                {organization.name || m.account_delete_organization_unnamed()}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    {/if}
    <!-- In the page from the start: a live region that appears with its words is often not read. -->
    <p class="text-sm text-destructive empty:hidden" role="alert">
      {failed ? m.account_leaving_cancel_failed() : ''}
    </p>
    <div class="flex flex-wrap justify-center gap-2">
      <Button
        bind:ref={cancelButton}
        class="roomy:min-h-touch"
        disabled={working}
        onclick={() => void cancel()}
      >
        {working ? m.account_working() : m.account_leaving_cancel()}
      </Button>
      <Button variant="outline" class="roomy:min-h-touch" onclick={onSignOut}>
        {m.account_sign_out()}
      </Button>
    </div>
  </div>
</main>
