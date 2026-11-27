<!--
  How a new account is used, in the form that makes it: on one's own (the
  first choice: the account starts in its own space), or together with
  others, which asks for the name of the organization they work in and starts
  it with the account. Where the server needs that name (`required`: its
  first account starts its organization) there is no choice, and the page
  says why the name is asked.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  interface Props {
    /** Working together is chosen; a required name is asked whatever was chosen. */
    together: boolean;
    name: string;
    required: boolean;
    /** What is wrong with the name, as the server or the page said. */
    problem?: string | undefined;
  }

  let { together = $bindable(), name = $bindable(), required, problem }: Props = $props();

  const uid = $props.id();
  const asked = $derived(required || together);
</script>

{#if required}
  <p class="text-sm text-muted-foreground" data-testid="register-first">
    {m.account_register_first()}
  </p>
{:else}
  <fieldset class="grid gap-2" data-testid="register-use">
    <legend class="mb-1 text-sm font-medium">{m.account_register_use()}</legend>
    {@render choice(false, m.account_register_solo(), m.account_register_solo_hint())}
    {@render choice(true, m.account_register_together(), m.account_register_together_hint())}
  </fieldset>
{/if}

{#snippet choice(shared: boolean, label: string, hint: string)}
  <label
    class="flex min-h-8 items-start gap-3 rounded-md border px-3 py-2 has-checked:border-primary roomy:min-h-touch"
  >
    <input
      type="radio"
      name="{uid}-use"
      value={shared ? 'together' : 'solo'}
      checked={together === shared}
      onchange={() => {
        together = shared;
      }}
      data-testid={shared ? 'register-together' : 'register-solo'}
      class="mt-1 size-4 accent-primary"
    />
    <span class="grid gap-0.5">
      <span class="text-sm font-medium">{label}</span>
      <span class="text-xs text-muted-foreground">{hint}</span>
    </span>
  </label>
{/snippet}

{#if asked}
  <div class="grid gap-1.5">
    <Label for="account-organizationName">{m.account_organization_name()}</Label>
    <Input
      id="account-organizationName"
      name="organizationName"
      data-testid="register-organization-name"
      bind:value={name}
      autocomplete="organization"
      required
      aria-invalid={problem ? 'true' : undefined}
      aria-describedby={problem
        ? 'account-organizationName-problem account-organizationName-hint'
        : 'account-organizationName-hint'}
      class="roomy:h-touch roomy:text-base"
    />
    {#if problem}
      <p
        id="account-organizationName-problem"
        class="text-sm text-destructive"
        role="alert"
        data-testid="problem-organizationName"
      >
        {problem}
      </p>
    {/if}
    <p id="account-organizationName-hint" class="text-xs text-muted-foreground">
      {m.account_organization_name_hint()}
    </p>
  </div>
{/if}
