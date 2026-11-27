<!--
  The reader's personal API tokens, in the settings: the keys they give an
  agent to work as them. "New token" unfolds the form (`ApiTokenForm.svelte`)
  under it; a token made is shown once, as the command that connects Claude
  Code with it and as itself, for another client, each copied by one press,
  with a line on the shell's history. Leaving it for a new token before
  either is copied is asked about first. The list says what each may do,
  where, until when and when it was last used, and ends one after asking.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import { lastUsedOf, scopeLabel } from '$lib/components/api-token/words.ts';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ApiToken, IssuedApiToken, NewApiToken } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import ApiTokenForm from './ApiTokenForm.svelte';
  import { mcpCommand, tokenProblemOf, type TokenWorkspace } from './api-tokens.ts';

  interface Props {
    /** The reader's tokens, the newest first. */
    read: () => Promise<ApiToken[]>;
    issue: (token: NewApiToken) => Promise<IssuedApiToken>;
    revoke: (tokenId: string) => Promise<void>;
    workspaces: readonly TokenWorkspace[];
    currentWorkspaceId: string | undefined;
    hasOwnSpace: boolean;
    hasPassword: boolean;
    /** The server's origin, which the command names. */
    origin: string;
  }

  let {
    read,
    issue,
    revoke,
    workspaces,
    currentWorkspaceId,
    hasOwnSpace,
    hasPassword,
    origin,
  }: Props = $props();

  const uid = $props.id();
  let tokens = $state.raw<ApiToken[] | undefined>(undefined);
  let unread = $state(false);
  let open = $state(false);
  let issued = $state.raw<IssuedApiToken | undefined>(undefined);
  /** What of the token made was copied last: the command, or the token itself. */
  let copied = $state<'command' | 'token' | undefined>(undefined);
  let status = $state('');
  let problem = $state('');
  let toggle = $state<HTMLElement | null>(null);
  /** What the reader is asked before it is done: ending a token, or leaving one made uncopied. */
  let asked = $state.raw<{ end: ApiToken } | 'leave' | undefined>(undefined);
  let asking = $state(false);

  const command = $derived(issued ? mcpCommand(origin, issued.token) : '');
  const dates = $derived(new Intl.DateTimeFormat(getLocale(), { dateStyle: 'medium' }));

  async function load(): Promise<void> {
    unread = false;
    try {
      tokens = await read();
    } catch {
      tokens = undefined;
      unread = true;
    }
  }
  onMount(() => {
    void load();
  });

  function made(token: IssuedApiToken): void {
    issued = token;
    open = false;
    copied = undefined;
    status = m.account_api_tokens_made();
    tokens = [token.apiToken, ...(tokens ?? [])];
  }

  /** The token made is where the focus goes: the form it came from is gone. */
  function focusOnShow(node: HTMLElement): void {
    node.focus();
  }

  /**
   * Copies the command or the token itself; where the browser lets no page write the
   * clipboard, says to copy it by hand.
   */
  async function copy(what: 'command' | 'token'): Promise<void> {
    problem = '';
    try {
      await navigator.clipboard.writeText(what === 'command' ? command : (issued?.token ?? ''));
      copied = what;
    } catch {
      problem = m.account_api_tokens_copy_failed();
    }
  }

  function toggleForm(): void {
    open = !open;
    issued = undefined;
    status = '';
  }

  function ask(what: { end: ApiToken } | 'leave'): void {
    asked = what;
    asking = true;
  }

  function done(): void {
    issued = undefined;
    status = '';
    toggle?.focus();
  }

  async function end(token: ApiToken): Promise<void> {
    problem = '';
    try {
      await revoke(token.id);
      tokens = (tokens ?? []).filter((one) => one.id !== token.id);
      if (issued?.apiToken.id === token.id) issued = undefined;
      status = m.account_api_tokens_revoked();
      toggle?.focus();
    } catch (error) {
      problem = tokenProblemOf(error).text;
    }
  }

  function where(token: ApiToken): string {
    const names = token.workspaces.map((one) => one.name);
    if (token.allowPersonalSpace) names.push(m.account_api_tokens_own_space_name());
    return names.join(', ');
  }
</script>

<section class="grid gap-3 border-t pt-4" aria-labelledby="{uid}-title" data-testid="api-tokens">
  <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
    <div class="grid min-w-0 flex-1 gap-0.5">
      <h3 id="{uid}-title" class="font-medium">{m.account_api_tokens_title()}</h3>
      <p class="text-xs text-muted-foreground">{m.account_api_tokens_hint()}</p>
    </div>
    <Button
      bind:ref={toggle}
      variant="outline"
      class="roomy:min-h-touch"
      aria-expanded={open}
      data-testid="api-token-new"
      onclick={() => {
        // A token made is not shown again: leaving it before it is copied is asked about.
        if (issued && copied === undefined) ask('leave');
        else toggleForm();
      }}
    >
      {open ? m.shared_cancel() : m.account_api_tokens_new()}
    </Button>
  </div>

  <!-- In the page from the start: a live region that appears with its words is often not read. -->
  <p class="text-sm text-muted-foreground empty:hidden" role="status">{status}</p>
  <p class="text-sm text-destructive empty:hidden" role="alert">{problem}</p>

  {#if open}
    <ApiTokenForm
      {workspaces}
      {currentWorkspaceId}
      {hasOwnSpace}
      {hasPassword}
      {issue}
      onIssued={made}
    />
  {/if}

  {#if issued}
    <div
      {@attach focusOnShow}
      tabindex="-1"
      class="grid gap-2 rounded-md border p-3 outline-none"
      data-testid="api-token-issued"
    >
      <p class="text-sm font-medium">{m.account_api_tokens_command()}</p>
      <code
        class="block overflow-x-auto rounded bg-muted p-2 font-mono text-xs break-all whitespace-pre-wrap"
        translate="no"
        data-testid="api-token-command">{command}</code
      >
      <p class="text-xs text-muted-foreground">{m.account_api_tokens_history()}</p>
      <Button
        class="justify-self-start roomy:min-h-touch"
        onclick={() => void copy('command')}
        data-testid="api-token-copy"
      >
        {copied === 'command' ? m.account_api_tokens_copied() : m.account_api_tokens_copy()}
      </Button>
      <p id="{uid}-raw" class="text-sm font-medium">{m.account_api_tokens_token()}</p>
      <div
        role="group"
        aria-labelledby="{uid}-raw"
        class="flex min-w-0 flex-wrap items-center gap-2"
      >
        <code
          class="min-w-0 flex-1 overflow-x-auto rounded bg-muted p-2 font-mono text-xs break-all"
          translate="no"
          data-testid="api-token-raw">{issued.token}</code
        >
        <Button
          variant="outline"
          class="roomy:min-h-touch"
          onclick={() => void copy('token')}
          data-testid="api-token-copy-token"
        >
          {copied === 'token' ? m.account_api_tokens_copied() : m.account_api_tokens_copy_token()}
        </Button>
      </div>
      <Button variant="outline" class="justify-self-start roomy:min-h-touch" onclick={done}>
        {m.account_api_tokens_done()}
      </Button>
    </div>
  {/if}

  {#if unread}
    <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
      <p class="min-w-0 flex-1 text-sm" role="alert">{m.app_problem_load()}</p>
      <Button variant="outline" class="roomy:min-h-touch" onclick={() => void load()}>
        {m.app_retry()}
      </Button>
    </div>
  {:else if tokens === undefined}
    <p class="text-sm text-muted-foreground">{m.app_loading()}</p>
  {:else if tokens.length === 0}
    <p class="text-sm text-muted-foreground">{m.account_api_tokens_none()}</p>
  {:else}
    <ul class="grid gap-2">
      {#each tokens as token (token.id)}
        <li class="flex min-h-8 items-center gap-3 roomy:min-h-touch" data-testid="api-token">
          <span class="grid min-w-0 flex-1">
            <span class="truncate">{token.name}</span>
            <span class="truncate text-xs text-muted-foreground">
              {scopeLabel(token.scope)} · {where(token)}
            </span>
            <span class="truncate text-xs text-muted-foreground">
              <span translate="no">{token.tokenPrefix}…</span> ·
              {m.account_api_tokens_expires({ date: dates.format(token.expiresAt) })} ·
              {lastUsedOf(token, dates)}
            </span>
          </span>
          <Button
            variant="outline"
            class="roomy:min-h-touch"
            aria-label="{m.account_api_tokens_revoke()}: {token.name}"
            onclick={() => {
              ask({ end: token });
            }}
          >
            {m.account_api_tokens_revoke()}
          </Button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<ConfirmDialog
  bind:open={asking}
  title={asked === 'leave'
    ? m.account_api_tokens_leave_title()
    : m.account_api_tokens_revoke_title({ name: asked?.end.name ?? '' })}
  description={asked === 'leave'
    ? m.account_api_tokens_leave_info()
    : m.account_api_tokens_revoke_info()}
  confirmLabel={asked === 'leave' ? m.account_api_tokens_new() : m.account_api_tokens_revoke()}
  destructive={asked !== 'leave'}
  focusAfter={() => toggle}
  onConfirm={() => {
    if (asked === 'leave') toggleForm();
    else if (asked) void end(asked.end);
  }}
/>
