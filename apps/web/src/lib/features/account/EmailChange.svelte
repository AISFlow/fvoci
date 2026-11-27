<!--
  The address of the reader's account, in the settings, and changing it.
  Folded until asked for: the new address and the account's password, which
  the server asks for in that very request. Where the server mails, the
  address then waits (`pending`): the link mailed to the new one changes it,
  and until then the section says so and offers to mail it again or to take
  the asking back. A server without mail (`mails` false) changes it at once,
  and the form says so. The server does
  not say whether an address is somebody else's: the account waits all the
  same, and no link comes.

  An account without a password has nothing to ask with: it sets one first.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { emailChangeProblemOf, type EmailChangeProblem } from './email-change.ts';

  interface Props {
    /** The account's address; empty for an account that has none. */
    email: string;
    /** The address the account waits to change to. */
    pending: string | undefined;
    /** Whether the account has a password to ask with. */
    hasPassword: boolean;
    /** Whether the server sends mail: the link, and the mail that resets a password. */
    mails: boolean;
    /** Asks for the change; resolved, `email` and `pending` are as the server has them. */
    onRequest: (email: string, password: string) => Promise<void>;
    /** Mails the link again. */
    onResend: () => Promise<void>;
    /** Takes the asking back; resolved, `pending` is as the server has it. */
    onCancel: () => Promise<void>;
  }

  let { email, pending, hasPassword, mails, onRequest, onResend, onCancel }: Props = $props();

  const uid = $props.id();
  let open = $state(false);
  let next = $state('');
  let password = $state('');
  let working = $state(false);
  let problem = $state.raw<EmailChangeProblem | undefined>(undefined);
  /** What was done last, said once below the address. */
  let said = $state<'asked' | 'changed' | 'resent' | 'cancelled' | 'gone' | undefined>(undefined);
  let toggle = $state<HTMLElement | null>(null);
  let nextField = $state<HTMLInputElement | null>(null);
  let passwordField = $state<HTMLInputElement | null>(null);

  const status = $derived.by(() => {
    if (said === 'asked') return m.email_change_asked();
    if (said === 'changed') return m.email_change_done();
    if (said === 'resent') return m.email_change_resent();
    if (said === 'cancelled') return m.email_change_cancelled();
    if (said === 'gone') return m.email_change_gone();
    return '';
  });

  function fold(): void {
    open = false;
    next = '';
    password = '';
    problem = undefined;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    said = undefined;
    const asked = next;
    try {
      await onRequest(asked, password);
      fold();
      // No link to wait for: the server changed it with the password alone.
      if (pending === undefined && email.toLowerCase() === asked.trim().toLowerCase()) {
        said = 'changed';
      } else if (pending !== undefined) {
        said = 'asked';
      }
      toggle?.focus();
    } catch (error) {
      problem = emailChangeProblemOf(error, asked, email);
      if (problem.field === 'email') nextField?.focus();
      else if (problem.field === 'password') passwordField?.focus();
    } finally {
      working = false;
    }
  }

  /** Mails the link again, or takes the asking back: what the waiting change offers. */
  async function act(does: 'resent' | 'cancelled'): Promise<void> {
    if (working) return;
    working = true;
    problem = undefined;
    said = undefined;
    try {
      await (does === 'resent' ? onResend() : onCancel());
      said = does;
    } catch (error) {
      // Nothing waits any more: confirmed or taken back elsewhere, or ended.
      if (isBackendError(error) && error.status === 404) said = 'gone';
      else problem = emailChangeProblemOf(error, '', email);
    } finally {
      working = false;
      // The buttons of a change that waits no more are gone: the focus is not left on nothing.
      if (pending === undefined) toggle?.focus();
    }
  }
</script>

<div class="grid gap-3 border-t pt-4" data-testid="email-change">
  <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
    <div class="grid min-w-0 flex-1">
      <span class="font-medium">{m.email_change_title()}</span>
      <span class="truncate text-sm text-muted-foreground" data-testid="email-change-address">
        {email || m.email_change_none()}
      </span>
    </div>
    {#if hasPassword}
      <Button
        bind:ref={toggle}
        variant="outline"
        class="roomy:min-h-touch"
        aria-expanded={open}
        aria-controls={open ? `${uid}-form` : undefined}
        onclick={() => {
          if (open) fold();
          else open = true;
          said = undefined;
        }}
      >
        {open ? m.shared_cancel() : m.email_change_open()}
      </Button>
    {/if}
  </div>

  {#if !hasPassword}
    <p class="text-sm text-muted-foreground">{m.email_change_no_password()}</p>
  {/if}

  {#if hasPassword && mails && email}
    <p class="text-xs text-muted-foreground" data-testid="email-change-revert-hint">
      {m.email_change_revert_hint()}
    </p>
  {/if}

  {#if pending !== undefined}
    <div class="grid gap-2 rounded-md border p-3" data-testid="email-change-pending">
      <p class="text-sm">{m.email_change_pending({ email: pending })}</p>
      <div class="flex flex-wrap gap-2">
        <Button
          variant="outline"
          class="roomy:min-h-touch"
          disabled={working}
          onclick={() => void act('resent')}
        >
          {m.email_change_resend()}
        </Button>
        <Button
          variant="ghost"
          class="roomy:min-h-touch"
          disabled={working}
          onclick={() => void act('cancelled')}
        >
          {m.email_change_cancel()}
        </Button>
      </div>
    </div>
  {/if}

  <!--
    In the page from the start: a live region that appears with its words is often not read.
    That the change was asked for is said to a screen reader only: the box above says it to the eye.
  -->
  <p
    class={['text-sm text-muted-foreground empty:hidden', said === 'asked' && 'sr-only']}
    role="status"
  >
    {status}
  </p>

  {#if open}
    <form id="{uid}-form" class="grid gap-3" onsubmit={(event) => void submit(event)}>
      <div class="grid gap-1.5">
        <Label for="{uid}-new">{m.email_change_new()}</Label>
        <Input
          bind:ref={nextField}
          id="{uid}-new"
          type="email"
          bind:value={next}
          autocomplete="email"
          required
          aria-invalid={problem?.field === 'email' ? 'true' : undefined}
          aria-describedby={problem?.field === 'email'
            ? `${uid}-problem ${uid}-hint`
            : `${uid}-hint`}
          class="roomy:h-touch roomy:text-base"
        />
        <p id="{uid}-hint" class="text-xs text-muted-foreground">
          {mails ? m.email_change_hint() : m.email_change_hint_now()}
        </p>
      </div>
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
      <Button type="submit" class="justify-self-start roomy:min-h-touch" disabled={working}>
        {working ? m.account_working() : m.email_change_submit()}
      </Button>
    </form>
  {/if}
  {#if problem}
    <p id="{uid}-problem" class="text-sm text-destructive" role="alert">{problem.text}</p>
  {/if}
</div>
