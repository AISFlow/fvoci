<!--
  What a workspace of an organization that takes no password shows to
  somebody whose session was made another way: that it is opened by signing
  in through the organization's single sign-on, and the button that goes
  there. The session stays as it is, and the reader's workspaces that it
  opens are listed below: the shell has no other way from here to them.

  Whoever has not connected single sign-on to their account yet is told
  where that is done: the provider would otherwise refuse them for their
  address being an account's already.
-->
<script lang="ts">
  import { nameOfWorkspace } from '$lib/components/workspace-name.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { OrgKind } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    /** The workspace's name; empty if it has none. */
    name: string;
    /** The reader's workspaces that are open to this session: where else they can go. */
    others: readonly { id: string; title: string; kind?: OrgKind; href: string }[];
    /** Starts the sign-in and sends the browser to the provider; a refusal rejects. */
    onContinue: () => Promise<void>;
    /** Opens the connected sign-ins of the settings. */
    onOpenLinked: () => void;
    /** Goes to another of the reader's workspaces. */
    onGo: (workspaceId: string) => void;
  }

  let { name, others, onContinue, onOpenLinked, onGo }: Props = $props();

  const uid = $props.id();
  let working = $state(false);
  let failed = $state(false);

  async function go(): Promise<void> {
    if (working) return;
    working = true;
    failed = false;
    try {
      // The browser leaves for the provider: the button stays as it is.
      await onContinue();
    } catch {
      failed = true;
      working = false;
    }
  }

  /** A plain click goes there in the page; any other is the browser's (a new tab). */
  function follow(event: MouseEvent, workspaceId: string): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onGo(workspaceId);
  }
</script>

<!-- Back from the provider by the browser's "back", the page is as it was left: not on its way any more. -->
<svelte:window
  onpageshow={(event: PageTransitionEvent) => {
    if (event.persisted) working = false;
  }}
/>

<div class="grid h-full place-items-center p-6 text-center" data-testid="sso-required">
  <div class="grid max-w-md justify-items-center gap-3">
    <h2 class="text-lg font-medium">
      {name ? m.account_sso_required_title_named({ name }) : m.account_sso_required_title()}
    </h2>
    <p class="text-muted-foreground">{m.account_sso_required_text()}</p>
    <Button
      class="roomy:min-h-touch"
      disabled={working}
      data-testid="sso-continue"
      onclick={() => void go()}
    >
      {working ? m.account_working() : m.account_sso_continue()}
    </Button>
    <!-- In the page from the start: a live region that appears with its words is often not read. -->
    <p class="text-sm text-destructive empty:hidden" role="alert">
      {failed ? m.account_oidc_failed() : ''}
    </p>
    <p class="text-sm text-muted-foreground">
      {m.account_sso_first_time()}
      <button
        type="button"
        class="underline underline-offset-4 roomy:py-3"
        data-testid="sso-open-linked"
        onclick={onOpenLinked}>{m.account_linked_open()}</button
      >
    </p>
    {#if others.length > 0}
      <nav class="grid gap-1 border-t pt-3 text-sm" aria-labelledby="{uid}-others">
        <p id="{uid}-others" class="text-muted-foreground">{m.account_sso_others()}</p>
        <ul class="grid gap-1">
          {#each others as other (other.id)}
            <li>
              <a
                class="underline underline-offset-4 roomy:inline-block roomy:py-3"
                href={other.href}
                onclick={(event) => {
                  follow(event, other.id);
                }}>{nameOfWorkspace(other, m.account_sso_other_unnamed)}</a
              >
            </li>
          {/each}
        </ul>
      </nav>
    {/if}
  </div>
</div>
