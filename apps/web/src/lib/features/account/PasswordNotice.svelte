<!--
  The notice for an account that signs in through its organization's provider
  only, in its own space: why to set a password (should that sign-in stop
  working, nothing else opens the own space; once set, the own space opens
  only with it), and the password set from here (`onSet`). It does not take
  the focus: it stands at the foot of the pane, and the page goes on.

  A session too old to vouch for the account is refused: the notice says to
  sign in through the provider again and offers that (`onSignInAgain`, which
  comes back to the page). "Don't show again" closes it for good
  (`onDismiss`); set, it says how to sign in from now on and closes.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import KeyRoundIcon from '@lucide/svelte/icons/key-round';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { setPasswordProblemOf, type SetPasswordProblem } from './password-notice.ts';

  interface Props {
    /** The name the account signs in with by its password from now on. */
    username: string;
    /** Sets the account's first password; a refusal rejects. */
    onSet: (password: string) => Promise<void>;
    /** Sends the browser through the provider and back; left out where none is known. */
    onSignInAgain?: (() => Promise<void>) | undefined;
    /** Closes the notice for good. */
    onDismiss: () => void;
    /** Closes the notice once it has said that the password is set. */
    onClose: () => void;
  }

  let { username, onSet, onSignInAgain, onDismiss, onClose }: Props = $props();

  const uid = $props.id();
  let password = $state('');
  let working = $state(false);
  let problem = $state.raw<SetPasswordProblem | undefined>(undefined);
  let done = $state(false);
  let field = $state<HTMLInputElement | null>(null);
  let closeButton = $state<HTMLElement | null>(null);

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    try {
      await onSet(password);
      password = '';
      done = true;
      await tick();
      closeButton?.focus();
    } catch (error) {
      problem = setPasswordProblemOf(error);
      // The form gives way to what is left to do: the field again, or closing.
      await tick();
      (problem.field ? field : problem.had ? closeButton : null)?.focus();
    } finally {
      working = false;
    }
  }

  async function signInAgain(): Promise<void> {
    if (!onSignInAgain || working) return;
    working = true;
    try {
      // Started, the browser is leaving for the provider: the notice stays busy.
      await onSignInAgain();
    } catch {
      problem = { text: m.account_oidc_failed(), field: false, again: true, had: false };
      working = false;
    }
  }
</script>

<section
  class="grid w-full max-w-md gap-3 rounded-lg border bg-popover p-4 text-popover-foreground shadow-lg"
  aria-labelledby="{uid}-title"
  data-testid="password-notice"
>
  <h2 id="{uid}-title" class="flex items-center gap-2 font-semibold">
    <KeyRoundIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    {m.password_notice_title()}
  </h2>
  {#if done || problem?.had}
    <p class="text-sm" role="status" data-testid="password-notice-done">
      {done ? m.password_notice_done({ username }) : m.password_notice_already()}
    </p>
    <Button bind:ref={closeButton} class="justify-self-end roomy:min-h-touch" onclick={onClose}>
      {m.ui_close()}
    </Button>
  {:else}
    <p class="text-sm text-muted-foreground">{m.password_notice_text()}</p>
    <form class="grid gap-2" onsubmit={(event) => void submit(event)}>
      <Label for="{uid}-password">{m.account_password_new()}</Label>
      <Input
        bind:ref={field}
        id="{uid}-password"
        type="password"
        bind:value={password}
        autocomplete="new-password"
        required
        aria-invalid={problem?.field ? 'true' : undefined}
        aria-describedby={problem ? `${uid}-problem ${uid}-hint` : `${uid}-hint`}
        data-testid="password-notice-field"
        class="roomy:h-touch roomy:text-base"
      />
      <p id="{uid}-hint" class="text-xs text-muted-foreground">{m.account_password_hint()}</p>
      <p
        id="{uid}-problem"
        class="text-sm text-destructive empty:hidden"
        role="alert"
        data-testid="password-notice-problem"
      >
        {problem?.text ?? ''}
      </p>
      {#if problem?.again && onSignInAgain}
        <Button
          type="button"
          variant="outline"
          class="roomy:min-h-touch"
          disabled={working}
          data-testid="password-notice-provider"
          onclick={() => void signInAgain()}
        >
          {m.password_notice_to_provider()}
        </Button>
      {/if}
      <div class="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          class="roomy:min-h-touch"
          disabled={working}
          data-testid="password-notice-dismiss"
          onclick={onDismiss}
        >
          {m.password_notice_dismiss()}
        </Button>
        <Button type="submit" class="roomy:min-h-touch" disabled={working}>
          {working ? m.account_working() : m.password_notice_set()}
        </Button>
      </div>
    </form>
  {/if}
</section>
