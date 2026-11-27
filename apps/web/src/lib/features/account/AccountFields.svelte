<!--
  The fields of a new account: the name to sign in with, the name to be shown
  by, the email address and the password. The page that starts an organization and the
  page of an invitation both make an account with them.

  The length of a password is the server's to check (it counts characters, a
  browser's `minlength` counts UTF-16 units).
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import type { FieldProblems } from './problems.ts';

  interface Props {
    username: string;
    displayName: string;
    email: string;
    password: string;
    /** What is wrong with each field, said beside it. */
    problems: FieldProblems;
  }

  let {
    username = $bindable(),
    displayName = $bindable(),
    email = $bindable(),
    password = $bindable(),
    problems,
  }: Props = $props();
</script>

{#snippet problemOf(field: 'username' | 'displayName' | 'email' | 'password')}
  {#if problems[field]}
    <p
      id="account-{field}-problem"
      class="text-sm text-destructive"
      role="alert"
      data-testid="problem-{field}"
    >
      {problems[field]}
    </p>
  {/if}
{/snippet}

<div class="grid gap-1.5">
  <Label for="account-username">{m.account_username()}</Label>
  <Input
    id="account-username"
    name="username"
    bind:value={username}
    autocomplete="username"
    required
    aria-invalid={problems.username ? 'true' : undefined}
    aria-describedby={problems.username ? 'account-username-problem' : undefined}
    class="roomy:h-touch roomy:text-base"
  />
  {@render problemOf('username')}
</div>
<div class="grid gap-1.5">
  <Label for="account-displayName">{m.account_display_name()}</Label>
  <Input
    id="account-displayName"
    name="displayName"
    bind:value={displayName}
    autocomplete="name"
    aria-invalid={problems.displayName ? 'true' : undefined}
    aria-describedby={problems.displayName
      ? 'account-displayName-problem account-displayName-hint'
      : 'account-displayName-hint'}
    class="roomy:h-touch roomy:text-base"
  />
  {@render problemOf('displayName')}
  <p id="account-displayName-hint" class="text-xs text-muted-foreground">
    {m.account_display_name_hint()}
  </p>
</div>
<div class="grid gap-1.5">
  <Label for="account-email">{m.account_email()}</Label>
  <Input
    id="account-email"
    name="email"
    type="email"
    bind:value={email}
    autocomplete="email"
    required
    aria-invalid={problems.email ? 'true' : undefined}
    aria-describedby={problems.email ? 'account-email-problem' : undefined}
    class="roomy:h-touch roomy:text-base"
  />
  {@render problemOf('email')}
</div>
<div class="grid gap-1.5">
  <Label for="account-password">{m.account_password()}</Label>
  <Input
    id="account-password"
    name="password"
    type="password"
    bind:value={password}
    autocomplete="new-password"
    required
    aria-invalid={problems.password ? 'true' : undefined}
    aria-describedby={problems.password ? 'account-password-problem' : 'account-password-hint'}
    class="roomy:h-touch roomy:text-base"
  />
  {@render problemOf('password')}
  <!-- What is wrong with the password says what the hint would say again. -->
  {#if !problems.password}
    <p id="account-password-hint" class="text-xs text-muted-foreground">
      {m.account_password_hint()}
    </p>
  {/if}
</div>
