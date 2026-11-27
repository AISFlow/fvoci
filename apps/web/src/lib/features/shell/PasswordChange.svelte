<!--
  Changing the reader's password, in the settings. Folded until asked for:
  the current password and the new one. The server checks both (the length
  too: it counts characters, a browser's `minlength` counts UTF-16 units);
  a refusal is told by its reason (`password-change.ts`), at the field it is
  about. Once it is changed, the reader's other sessions are over, and this
  one goes on; the form folds and the focus goes back to its button.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { passwordProblemOf, type PasswordProblem } from './password-change.ts';

  interface Props {
    onChange: (oldPassword: string, newPassword: string) => Promise<void>;
  }

  let { onChange }: Props = $props();

  const uid = $props.id();
  let open = $state(false);
  let current = $state('');
  let next = $state('');
  let working = $state(false);
  let problem = $state.raw<PasswordProblem | undefined>(undefined);
  let done = $state(false);
  let toggle = $state<HTMLElement | null>(null);
  let currentField = $state<HTMLInputElement | null>(null);
  let nextField = $state<HTMLInputElement | null>(null);

  function fold(): void {
    open = false;
    current = '';
    next = '';
    problem = undefined;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    done = false;
    try {
      await onChange(current, next);
      fold();
      done = true;
      toggle?.focus();
    } catch (error) {
      problem = passwordProblemOf(error, next);
      if (problem.field === 'current') currentField?.focus();
      else if (problem.field === 'next') nextField?.focus();
    } finally {
      working = false;
    }
  }
</script>

<div class="grid gap-3 border-t pt-4" data-testid="password-change">
  <div class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
    <span class="min-w-0 flex-1 font-medium">{m.account_password_change()}</span>
    <Button
      bind:ref={toggle}
      variant="outline"
      class="roomy:min-h-touch"
      aria-expanded={open}
      aria-controls={open ? `${uid}-form` : undefined}
      onclick={() => {
        if (open) fold();
        else open = true;
        done = false;
      }}
    >
      {open ? m.shared_cancel() : m.account_password_change()}
    </Button>
  </div>

  <!-- In the page from the start: a live region that appears with its words is often not read. -->
  <p class="text-sm text-muted-foreground empty:hidden" role="status">
    {done ? m.account_password_changed() : ''}
  </p>

  {#if open}
    <form id="{uid}-form" class="grid gap-3" onsubmit={(event) => void submit(event)}>
      <div class="grid gap-1.5">
        <Label for="{uid}-current">{m.account_password_current()}</Label>
        <Input
          bind:ref={currentField}
          id="{uid}-current"
          type="password"
          bind:value={current}
          autocomplete="current-password"
          required
          aria-invalid={problem?.field === 'current' ? 'true' : undefined}
          aria-describedby={problem?.field === 'current' ? `${uid}-problem` : undefined}
          class="roomy:h-touch roomy:text-base"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="{uid}-new">{m.account_password_new()}</Label>
        <Input
          bind:ref={nextField}
          id="{uid}-new"
          type="password"
          bind:value={next}
          autocomplete="new-password"
          required
          aria-invalid={problem?.field === 'next' ? 'true' : undefined}
          aria-describedby={problem?.field === 'next'
            ? `${uid}-problem ${uid}-hint`
            : `${uid}-hint`}
          class="roomy:h-touch roomy:text-base"
        />
        <p id="{uid}-hint" class="text-xs text-muted-foreground">{m.account_password_hint()}</p>
      </div>
      {#if problem}
        <p id="{uid}-problem" class="text-sm text-destructive" role="alert">{problem.text}</p>
      {/if}
      <Button type="submit" class="justify-self-start roomy:min-h-touch" disabled={working}>
        {working ? m.account_working() : m.account_password_save()}
      </Button>
    </form>
  {/if}
</div>
