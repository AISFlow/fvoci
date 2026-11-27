<!--
  Making a personal API token, in the settings: one choice of what it may do
  (read only, or read and write content), a name it is given at first
  ("Agent · 11/27"), and the workspace shown, for 90 days. "More" unfolds the
  rest: the other workspaces of organizations the reader works in, their own
  space, which takes the account's password, and whether they are told of what
  the agent does (on at first). Folded again, it gives what it hid up: only
  what is shown is asked for. A refusal is told by its reason
  (`api-tokens.ts`), at the field it is about.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { scopeLabel } from '$lib/components/api-token/words.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ApiTokenScope, IssuedApiToken, NewApiToken } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Checkbox } from '$lib/ui/checkbox/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Switch } from '$lib/ui/switch/index.js';

  import {
    TOKEN_DAYS,
    defaultTokenName,
    tokenProblemOf,
    type TokenProblem,
    type TokenWorkspace,
  } from './api-tokens.ts';

  interface Props {
    /** The workspaces a token may be given. */
    workspaces: readonly TokenWorkspace[];
    /** The one shown, which a new token is given at first. */
    currentWorkspaceId: string | undefined;
    /** Whether the reader has an own space to give. */
    hasOwnSpace: boolean;
    /** Whether the account has a password, which giving the own space asks for. */
    hasPassword: boolean;
    issue: (token: NewApiToken) => Promise<IssuedApiToken>;
    onIssued: (issued: IssuedApiToken) => void;
  }

  let { workspaces, currentWorkspaceId, hasOwnSpace, hasPassword, issue, onIssued }: Props =
    $props();

  const SCOPES: readonly ApiTokenScope[] = ['read', 'write'];
  const uid = $props.id();
  let scope = $state<ApiTokenScope>('read');
  let name = $state(defaultTokenName(new Date()));
  // The form starts again whenever it is opened: the workspace shown then is the one given.
  // svelte-ignore state_referenced_locally
  let chosen = $state<string[]>(currentWorkspaceId ? [currentWorkspaceId] : []);
  let more = $state(false);
  let ownSpace = $state(false);
  let password = $state('');
  let notifySelf = $state(true);
  let working = $state(false);
  let problem = $state.raw<TokenProblem | undefined>(undefined);
  let nameField = $state<HTMLInputElement | null>(null);
  let passwordField = $state<HTMLInputElement | null>(null);

  const current = $derived(workspaces.find((one) => one.id === currentWorkspaceId));
  const others = $derived(workspaces.filter((one) => one.id !== currentWorkspaceId));
  const nothingChosen = $derived(chosen.length === 0 && !ownSpace);

  function choose(id: string, on: boolean): void {
    chosen = on ? [...chosen, id] : chosen.filter((one) => one !== id);
  }

  /** Folds "More" and gives up what it held: a choice the reader cannot see is not made. */
  function fold(): void {
    more = false;
    chosen = chosen.filter((one) => one === currentWorkspaceId);
    ownSpace = false;
    password = '';
    notifySelf = true;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working || nothingChosen) return;
    working = true;
    problem = undefined;
    try {
      onIssued(
        await issue({
          name: name.trim(),
          scope,
          workspaceIds: chosen,
          expiresInDays: TOKEN_DAYS,
          allowPersonalSpace: ownSpace,
          password: ownSpace && hasPassword ? password : undefined,
          notifySelf,
        }),
      );
    } catch (error) {
      problem = tokenProblemOf(error);
      await tick();
      if (problem.field === 'name') nameField?.focus();
      else if (problem.field === 'password') passwordField?.focus();
    } finally {
      working = false;
    }
  }
</script>

<form class="grid gap-4" data-testid="api-token-form" onsubmit={(event) => void submit(event)}>
  <fieldset class="grid gap-2" data-setting="apiTokenScope">
    <legend class="mb-1 text-sm font-medium">{m.account_api_tokens_scope()}</legend>
    {#each SCOPES as one (one)}
      <label class="flex min-h-8 items-center gap-2 roomy:min-h-touch">
        <input type="radio" name="{uid}-scope" value={one} bind:group={scope} class="size-4" />
        <span>{scopeLabel(one)}</span>
      </label>
    {/each}
  </fieldset>
  <div class="grid gap-1.5">
    <Label for="{uid}-name">{m.account_api_tokens_name()}</Label>
    <Input
      bind:ref={nameField}
      id="{uid}-name"
      bind:value={name}
      maxlength={80}
      required
      aria-invalid={problem?.field === 'name' ? 'true' : undefined}
      aria-describedby={problem?.field === 'name' ? `${uid}-problem` : undefined}
      class="roomy:h-touch roomy:text-base"
    />
  </div>
  <p class="text-xs text-muted-foreground">
    {#if current}{m.account_api_tokens_current({ name: current.name })}{/if}
    {m.account_api_tokens_days({ days: String(TOKEN_DAYS) })}
  </p>

  <div class="grid gap-3">
    <Button
      variant="ghost"
      class="justify-self-start roomy:min-h-touch"
      aria-expanded={more}
      aria-controls={more ? `${uid}-more` : undefined}
      onclick={() => {
        if (more) fold();
        else more = true;
      }}>{m.account_api_tokens_more()}</Button
    >
    {#if more}
      <div id="{uid}-more" class="grid gap-3 border-l pl-3">
        {#if others.length > 0}
          <fieldset class="grid gap-2">
            <legend class="mb-1 text-sm font-medium">{m.account_api_tokens_others()}</legend>
            {#each others as one (one.id)}
              <label class="flex min-h-8 items-center gap-2 roomy:min-h-touch">
                <Checkbox
                  checked={chosen.includes(one.id)}
                  onCheckedChange={(on: boolean) => {
                    choose(one.id, on);
                  }}
                />
                <span class="min-w-0 truncate">{one.name}</span>
              </label>
            {/each}
          </fieldset>
        {/if}
        {#if hasOwnSpace}
          <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
            <span class="min-w-0 flex-1">{m.account_api_tokens_own_space()}</span>
            <Switch
              checked={ownSpace}
              onCheckedChange={(on: boolean) => {
                ownSpace = on;
              }}
            />
          </label>
          {#if ownSpace && hasPassword}
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
                aria-describedby={problem?.field === 'password'
                  ? `${uid}-password-hint ${uid}-problem`
                  : `${uid}-password-hint`}
                class="roomy:h-touch roomy:text-base"
              />
              <p id="{uid}-password-hint" class="text-xs text-muted-foreground">
                {m.account_api_tokens_own_space_password()}
              </p>
            </div>
          {/if}
        {/if}
        <div class="grid gap-1">
          <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
            <span class="min-w-0 flex-1">{m.account_api_tokens_notify_self()}</span>
            <Switch
              checked={notifySelf}
              aria-describedby="{uid}-notify-hint"
              onCheckedChange={(on: boolean) => {
                notifySelf = on;
              }}
            />
          </label>
          <p id="{uid}-notify-hint" class="text-xs text-muted-foreground">
            {m.account_api_tokens_notify_self_hint()}
          </p>
        </div>
      </div>
    {/if}
  </div>

  <!-- In the page from the start: a live region that appears with its words is often not read. -->
  <p id="{uid}-problem" class="text-sm text-destructive empty:hidden" role="alert">
    {problem?.text ?? ''}
  </p>
  {#if nothingChosen}
    <p id="{uid}-none" class="text-xs text-muted-foreground">
      {m.account_api_tokens_none_chosen()}
    </p>
  {/if}
  <Button
    type="submit"
    class="justify-self-start roomy:min-h-touch"
    disabled={working || nothingChosen}
    aria-describedby={nothingChosen ? `${uid}-none` : undefined}
    data-testid="api-token-make"
  >
    {working ? m.account_working() : m.account_api_tokens_make()}
  </Button>
</form>
