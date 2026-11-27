<!--
  The reader's degrees and qualifications, in the settings: up to three from
  the list, written after the name in the order they were added. Each listed
  one has its button to remove it; one is added by choosing it in the list
  and pressing "Add" (choosing alone adds nothing, so that the arrow keys can
  go through the list). The list leaves out those listed, and is off once
  three are. Focus is never left on a button that goes away or turns off: a
  removal moves it to the next one listed, or to the list; an addition back
  to the list, or to the last one listed once three are.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { CREDENTIALS, MAX_CREDENTIALS, type Credential } from '$lib/logic/person-names.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { CREDENTIAL_LABELS } from '$lib/components/person/name-labels.ts';

  let {
    value = $bindable(),
    disabled = false,
    problem,
    onchange,
  }: {
    value: Credential[];
    disabled?: boolean;
    problem?: string;
    onchange?: () => void;
  } = $props();

  const uid = $props.id();
  let chosen = $state('');
  let listed = $state<HTMLOListElement>();
  let picker = $state<HTMLSelectElement>();

  const full = $derived(value.length >= MAX_CREDENTIALS);
  const left = $derived(CREDENTIALS.filter((code) => !value.includes(code)));

  /** The remove button of the credential at `index`, as drawn now. */
  function removeButton(index: number): HTMLButtonElement | undefined {
    return listed?.querySelectorAll<HTMLButtonElement>('button')[index];
  }

  async function add(): Promise<void> {
    const known = left.find((credential) => credential === chosen);
    if (!known || full) return;
    value = [...value, known];
    chosen = '';
    onchange?.();
    await tick();
    // The button turns off with nothing chosen; the list does too once three are listed.
    if (value.length >= MAX_CREDENTIALS) removeButton(value.length - 1)?.focus();
    else picker?.focus();
  }

  async function remove(code: Credential): Promise<void> {
    const index = value.indexOf(code);
    value = value.filter((one) => one !== code);
    onchange?.();
    await tick();
    (removeButton(index) ?? removeButton(index - 1) ?? picker)?.focus();
  }
</script>

<fieldset class="grid gap-1.5" aria-describedby="{uid}-hint">
  <legend class="mb-1 font-medium">{m.account_name_credentials()}</legend>
  {#if value.length > 0}
    <ol class="flex flex-wrap gap-2" data-testid="credentials" bind:this={listed}>
      {#each value as code (code)}
        <li
          class="flex items-center gap-1 rounded-md border border-input-border py-0.5 ps-2"
          data-credential={code}
        >
          <span>{CREDENTIAL_LABELS[code]()}</span>
          <Button
            variant="ghost"
            size="sm"
            class="h-7 px-2 roomy:min-h-touch"
            {disabled}
            aria-label={m.account_name_credential_remove({ credential: CREDENTIAL_LABELS[code]() })}
            onclick={() => void remove(code)}
          >
            <span aria-hidden="true">×</span>
          </Button>
        </li>
      {/each}
    </ol>
  {/if}
  <div class="flex min-w-0 gap-2">
    <select
      id="{uid}-add"
      bind:this={picker}
      bind:value={chosen}
      class="h-8 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 roomy:h-touch"
      aria-label={m.account_name_credentials_add()}
      aria-invalid={problem ? 'true' : undefined}
      aria-describedby={problem ? `${uid}-problem` : undefined}
      disabled={disabled || full}
      data-testid="credential-add"
    >
      <option value="" disabled>{m.account_name_credentials_add()}</option>
      {#each left as code (code)}
        <option value={code}>{CREDENTIAL_LABELS[code]()}</option>
      {/each}
    </select>
    <Button
      variant="outline"
      size="sm"
      class="h-8 roomy:h-touch"
      disabled={disabled || full || chosen === ''}
      onclick={() => void add()}
    >
      {m.account_name_credentials_add_button()}
    </Button>
  </div>
  {#if problem}
    <p
      id="{uid}-problem"
      class="text-sm text-destructive"
      role="alert"
      data-testid="problem-credentials"
    >
      {problem}
    </p>
  {/if}
  <p id="{uid}-hint" class="text-xs text-muted-foreground" aria-live="polite">
    {full ? m.account_name_credentials_full() : m.account_name_credentials_hint()}
  </p>
</fieldset>
