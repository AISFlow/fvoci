<!--
  The sign-ins connected to the reader's account, in the settings: who they
  are at the single sign-on of their organizations, by which that sign-on
  signs in to this account. Connecting goes through an organization's provider
  and comes back; both connecting and disconnecting ask for the account's
  password, as changing the password does. A refusal is told by its reason
  (`linked-sign-ins.ts`).

  The account is one for every organization: a sign-in connected through the
  provider of one signs in to the whole account. The form says whose it is
  before the reader goes.

  One form is open at a time, under the button that opened it, which stays
  where it is and closes it again (as the form that changes the password
  does): the focus is in the password field while it is open, and back on
  that button after.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { LinkedIdentity, SignInOrganization } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { issuerHost, linkProblemOf } from './linked-sign-ins.ts';

  interface Props {
    /** The identities connected to the account, oldest first. */
    read: () => Promise<LinkedIdentity[]>;
    /** The reader's organizations that sign in through a provider. */
    organizations: () => Promise<SignInOrganization[]>;
    /** Starts connecting through the organization's provider and sends the browser there. */
    onLink: (orgId: string, password: string) => Promise<void>;
    onUnlink: (identity: LinkedIdentity, password: string) => Promise<void>;
    /** A sign-in was connected just now: said once. */
    justLinked?: boolean;
    /** Brings the section into view when it is shown: it was asked for by name. */
    reveal?: boolean;
  }

  let {
    read,
    organizations,
    onLink,
    onUnlink,
    justLinked = false,
    reveal = false,
  }: Props = $props();

  const uid = $props.id();
  /** What is connected; undefined until it is read, and when it could not be. */
  let identities = $state.raw<LinkedIdentity[] | undefined>(undefined);
  let offered = $state.raw<SignInOrganization[]>([]);
  let unread = $state(false);
  /** What is open: the form that connects, or the key of the identity being disconnected. */
  let open = $state<string | undefined>(undefined);
  let orgId = $state('');
  let password = $state('');
  let working = $state(false);
  let problem = $state<string | undefined>(undefined);
  let removed = $state(false);
  let section = $state<HTMLElement | null>(null);
  let passwordField = $state<HTMLInputElement | null>(null);

  const CONNECT = 'connect';
  const keyOf = (identity: LinkedIdentity): string => `${identity.issuer}\n${identity.subject}`;

  async function load(): Promise<void> {
    unread = false;
    try {
      [identities, offered] = await Promise.all([read(), organizations()]);
      if (!offered.some((one) => one.orgId === orgId)) orgId = offered[0]?.orgId ?? '';
    } catch {
      identities = undefined;
      unread = true;
    }
  }
  onMount(() => {
    if (reveal) section?.scrollIntoView({ block: 'nearest' });
    void load();
  });

  /** Opens the form of `next`, or closes the one that is open; the focus follows. */
  async function toggle(next: string, opener: HTMLElement): Promise<void> {
    const opening = open !== next;
    open = opening ? next : undefined;
    password = '';
    problem = undefined;
    removed = false;
    await tick();
    if (opening) passwordField?.focus();
    else opener.focus();
  }

  async function submit(event: SubmitEvent, identity: LinkedIdentity | undefined): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    try {
      if (identity === undefined) {
        // The browser goes to the provider: the page ends here, the button as it is.
        await onLink(orgId, password);
        return;
      }
      await onUnlink(identity, password);
      // This session ended with it: what reads next is refused, and the page
      // goes to signing in (`signedIn` in App.svelte).
      open = undefined;
      password = '';
      removed = true;
      working = false;
      await load();
      // The row is gone, and its button with it.
      section?.focus();
    } catch (error) {
      const told = linkProblemOf(error);
      problem = told.text;
      working = false;
      if (told.password) passwordField?.focus();
    }
  }
</script>

{#snippet passwordForm(action: string, identity: LinkedIdentity | undefined)}
  <form class="grid gap-3" onsubmit={(event) => void submit(event, identity)}>
    {#if identity?.madeThisSession}
      <!-- Said before it happens: the request that disconnects it ends this session. -->
      <p class="text-sm" data-testid="linked-ends-session">{m.account_linked_ends_session()}</p>
    {/if}
    <div class="grid gap-1.5">
      <Label for="{uid}-password">{m.account_password_current()}</Label>
      <Input
        bind:ref={passwordField}
        id="{uid}-password"
        type="password"
        bind:value={password}
        autocomplete="current-password"
        required
        aria-invalid={problem ? 'true' : undefined}
        aria-describedby="{uid}-problem"
        class="roomy:h-touch roomy:text-base"
      />
    </div>
    <!-- In the page from the start: a live region that appears with its words is often not read. -->
    <p id="{uid}-problem" class="text-sm text-destructive empty:hidden" role="alert">
      {problem ?? ''}
    </p>
    <Button type="submit" class="justify-self-start roomy:min-h-touch" disabled={working}>
      {working ? m.account_working() : action}
    </Button>
  </form>
{/snippet}

<!-- Back from the provider, the browser may show the page as it left it: usable again. -->
<svelte:window
  onpageshow={(event: PageTransitionEvent) => {
    if (event.persisted) working = false;
  }}
/>

<section
  bind:this={section}
  tabindex="-1"
  class="grid gap-3 border-t pt-4 outline-none"
  aria-labelledby="{uid}-title"
  data-testid="linked-sign-ins"
>
  <div class="grid gap-0.5">
    <h3 id="{uid}-title" class="font-medium">{m.account_linked_title()}</h3>
    <p class="text-xs text-muted-foreground">{m.account_linked_hint()}</p>
  </div>

  <!-- In the page from the start: a live region that appears with its words is often not read. -->
  <p class="text-sm text-muted-foreground empty:hidden" role="status">
    {#if removed}{m.account_linked_removed()}{:else if justLinked}{m.account_linked_done()}{/if}
  </p>

  {#if unread}
    <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
      <p class="min-w-0 flex-1 text-sm" role="alert">{m.app_problem_load()}</p>
      <Button variant="outline" class="roomy:min-h-touch" onclick={() => void load()}>
        {m.app_retry()}
      </Button>
    </div>
  {:else if identities === undefined}
    <p class="text-sm text-muted-foreground">{m.app_loading()}</p>
  {:else}
    {#if identities.length === 0}
      <p class="text-sm text-muted-foreground">{m.account_linked_none()}</p>
    {:else}
      <ul class="grid gap-2">
        {#each identities as identity (keyOf(identity))}
          {@const key = keyOf(identity)}
          <li class="grid gap-2" data-testid="linked-sign-in">
            <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
              <span class="grid min-w-0 flex-1">
                <span class="truncate" translate="no">{issuerHost(identity.issuer)}</span>
                {#if identity.email}
                  <span class="truncate text-xs text-muted-foreground" translate="no">
                    {identity.email}
                  </span>
                {/if}
              </span>
              <Button
                variant="outline"
                class="roomy:min-h-touch"
                aria-expanded={open === key}
                onclick={(event: MouseEvent & { currentTarget: HTMLElement }) =>
                  void toggle(key, event.currentTarget)}
              >
                {open === key ? m.shared_cancel() : m.account_linked_disconnect()}
              </Button>
            </div>
            {#if open === key}
              {@render passwordForm(m.account_linked_disconnect_confirm(), identity)}
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if offered.length === 0}
      <p class="text-xs text-muted-foreground">{m.account_linked_none_offered()}</p>
    {:else}
      <Button
        variant="outline"
        class="justify-self-start roomy:min-h-touch"
        aria-expanded={open === CONNECT}
        data-testid="linked-connect"
        onclick={(event: MouseEvent & { currentTarget: HTMLElement }) =>
          void toggle(CONNECT, event.currentTarget)}
      >
        {open === CONNECT ? m.shared_cancel() : m.account_linked_connect()}
      </Button>
      {#if open === CONNECT}
        <div class="grid gap-3" data-testid="linked-connect-form">
          <div class="grid gap-1.5">
            <Label for="{uid}-which">{m.account_linked_which()}</Label>
            <select
              id="{uid}-which"
              bind:value={orgId}
              class="h-9 rounded-md border bg-background px-3 text-sm roomy:h-touch roomy:text-base"
            >
              {#each offered as one (one.orgId)}
                <option value={one.orgId}>{one.name}</option>
              {/each}
            </select>
            <p class="text-xs text-muted-foreground">{m.account_linked_whole_account()}</p>
          </div>
          {@render passwordForm(m.account_linked_continue(), undefined)}
        </div>
      {/if}
    {/if}
  {/if}
</section>
