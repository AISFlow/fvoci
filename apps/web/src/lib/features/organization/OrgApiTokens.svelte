<!--
  The personal API tokens of the organization's members that reach its
  workspaces, on the organization screen: whose each is, what it may do,
  where, until when and when it was last used. An administrator takes one
  out of the organization at once, after being asked; what it reaches
  elsewhere is its owner's to end.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import { lastUsedOf, scopeLabel } from '$lib/components/api-token/words.ts';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { MemberApiToken } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    read: () => Promise<MemberApiToken[]>;
    revoke: (tokenId: string) => Promise<void>;
    /** The name a member is shown by. */
    nameOf: (userId: string) => string;
  }

  let { read, revoke, nameOf }: Props = $props();

  let tokens = $state.raw<MemberApiToken[] | undefined>(undefined);
  let unread = $state(false);
  let problem = $state('');
  let ending = $state.raw<MemberApiToken | undefined>(undefined);
  let confirmOpen = $state(false);
  let heading: HTMLElement | null = null;

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

  async function end(token: MemberApiToken): Promise<void> {
    problem = '';
    try {
      await revoke(token.id);
    } catch {
      problem = m.org_refused_failed();
    }
    await load();
  }
</script>

<section class="flex flex-col gap-3" aria-labelledby="org-api-tokens" data-testid="org-api-tokens">
  <h2
    id="org-api-tokens"
    tabindex="-1"
    class="text-base font-semibold outline-none"
    {@attach (node: HTMLElement) => {
      heading = node;
      return () => {
        heading = null;
      };
    }}
  >
    {m.org_api_tokens_list()}
  </h2>
  <p class="text-sm text-destructive empty:hidden" role="alert">{problem}</p>
  {#if unread}
    <div class="flex items-center gap-3">
      <p class="min-w-0 flex-1 text-sm" role="alert">{m.app_problem_load()}</p>
      <Button variant="outline" class="roomy:min-h-touch" onclick={() => void load()}>
        {m.app_retry()}
      </Button>
    </div>
  {:else if tokens === undefined}
    <p class="text-sm text-muted-foreground">{m.app_loading()}</p>
  {:else if tokens.length === 0}
    <p class="text-sm text-muted-foreground">{m.org_api_tokens_none()}</p>
  {:else}
    <ul class="flex flex-col gap-2">
      {#each tokens as token (token.id)}
        <li
          class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2"
          data-testid="org-api-token"
        >
          <span class="grid min-w-0 flex-1">
            <span class="truncate">{token.name}</span>
            <span class="truncate text-xs text-muted-foreground">
              {m.org_api_tokens_owner({ name: nameOf(token.userId) })} · {scopeLabel(token.scope)} ·
              {token.workspaces.map((one) => one.name).join(', ')}
            </span>
            <span class="truncate text-xs text-muted-foreground">
              <span translate="no">{token.tokenPrefix}…</span> ·
              {m.account_api_tokens_expires({ date: dates.format(token.expiresAt) })} ·
              {lastUsedOf(token, dates)}
            </span>
          </span>
          <Button
            size="sm"
            variant="outline"
            aria-label="{m.org_api_tokens_revoke()}: {token.name}"
            onclick={() => {
              ending = token;
              confirmOpen = true;
            }}
          >
            {m.org_api_tokens_revoke()}
          </Button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<ConfirmDialog
  bind:open={confirmOpen}
  title={m.org_api_tokens_revoke_title({ name: ending?.name ?? '' })}
  description={m.org_api_tokens_revoke_info()}
  confirmLabel={m.org_api_tokens_revoke()}
  destructive
  focusAfter={() => heading}
  onConfirm={() => {
    if (ending) void end(ending);
  }}
/>
