<!--
  The reader's name, in the settings: its parts as they write it and in
  Latin letters, a name of their own, which of them others see (each choice
  shows the name it gives), a title and up to three degrees or
  qualifications. Saved with one button. A member the server refuses is told
  beside its field, by the reason it gives (`errors` at the member); any
  other failure is the page's problem line, and the form keeps what was
  typed.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    emptyName,
    NAME_SHOWN,
    nameOf,
    shownName,
    TITLES,
    type NameChange,
    type PersonName,
  } from '$lib/logic/person-names.ts';
  import { changeName, isBackendError, type Workspace } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import CredentialPicker from './CredentialPicker.svelte';
  import { NAME_SHOWN_LABELS, TITLE_LABELS } from '$lib/components/person/name-labels.ts';

  type Part = 'lastName' | 'firstName' | 'latinFirstName' | 'latinLastName' | 'customName';

  let { workspace }: { workspace: Workspace } = $props();

  const uid = $props.id();

  /** The name as it is stored; a user told without its parts has its display name as its own. */
  function storedOf(workspace: Workspace): PersonName {
    const me = workspace.me;
    if (!me) return emptyName();
    return me.name ?? { ...emptyName(), customName: me.displayName };
  }

  function copied(name: PersonName): PersonName {
    return { ...name, credentials: [...name.credentials] };
  }

  const stored = $derived(storedOf(workspace));
  // svelte-ignore state_referenced_locally (the form starts from the stored name, then is the reader's)
  let draft = $state<PersonName>(copied(stored));
  /**
   * The name the form started from: what the reader left as it was there is
   * not sent, so that a change made meanwhile elsewhere (another tab) stays.
   */
  // svelte-ignore state_referenced_locally (as the draft)
  let base = $state.raw<PersonName>(copied(stored));
  let working = $state(false);
  let saved = $state(false);
  let problems = $state<Partial<Record<Part | 'title' | 'credentials' | 'nameShown', string>>>({});

  const changed = $derived(JSON.stringify(draft) !== JSON.stringify(stored));
  const username = $derived(workspace.me?.username ?? '');
  const preview = $derived(shownName(draft) || username);

  function problemOf(reason: string): string {
    if (reason === 'too_long') return m.account_name_part_too_long();
    if (reason === 'invalid_characters') return m.account_name_part_invalid_characters();
    return m.account_name_refused();
  }

  /** The members of `draft` the reader changed from `from`. */
  function editedOf(draft: PersonName, from: PersonName): NameChange {
    const keys = Object.keys(draft) as (keyof PersonName)[];
    return Object.fromEntries(
      keys
        .filter((key) => JSON.stringify(draft[key]) !== JSON.stringify(from[key]))
        .map((key) => [key, draft[key]]),
    );
  }

  function edited(): void {
    saved = false;
    problems = {};
  }

  async function save(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    edited();
    try {
      await changeName(workspace, editedOf($state.snapshot(draft), base));
      // As the server keeps it: without the white space around each part.
      draft = copied(storedOf(workspace));
      base = copied(draft);
      saved = true;
    } catch (error) {
      const refused = isBackendError(error) ? error.errors : [];
      for (const entry of refused) {
        const [, member = ''] = (entry.pointer ?? '').split('/');
        if (member in draft) {
          problems = { ...problems, [member]: problemOf(entry.reason) };
        }
      }
      // A refusal of no member of the form (an older server's) is the page's to tell.
      if (Object.keys(problems).length === 0) workspace.report('save', error);
    } finally {
      working = false;
    }
  }
</script>

{#snippet part(member: Part, label: string, autocomplete: 'given-name' | 'family-name' | 'off')}
  <div class="grid gap-1.5">
    <Label for="{uid}-{member}">{label}</Label>
    <Input
      id="{uid}-{member}"
      name={member}
      bind:value={draft[member]}
      readonly={working}
      {autocomplete}
      oninput={edited}
      aria-invalid={problems[member] ? 'true' : undefined}
      aria-describedby={problems[member] ? `${uid}-${member}-problem` : undefined}
      class="roomy:h-touch roomy:text-base"
    />
    {#if problems[member]}
      <p
        id="{uid}-{member}-problem"
        class="text-sm text-destructive"
        role="alert"
        data-testid="problem-{member}"
      >
        {problems[member]}
      </p>
    {/if}
  </div>
{/snippet}

<form class="grid gap-4" data-setting="name" onsubmit={(event) => void save(event)}>
  <div class="grid gap-1">
    <h4 class="font-medium">{m.account_name()}</h4>
    <p class="text-xs text-muted-foreground">{m.account_name_hint()}</p>
  </div>
  <div class="grid gap-3 sm:grid-cols-2">
    {@render part('lastName', m.account_name_last(), 'family-name')}
    {@render part('firstName', m.account_name_first(), 'given-name')}
    {@render part('latinFirstName', m.account_name_latin_first(), 'off')}
    {@render part('latinLastName', m.account_name_latin_last(), 'off')}
  </div>
  {@render part('customName', m.account_name_custom(), 'off')}

  <fieldset class="grid gap-1">
    <legend class="mb-1 font-medium">{m.account_name_shown()}</legend>
    {#each NAME_SHOWN as shown (shown)}
      {@const gives = shown === 'username' ? username : nameOf(draft, shown)}
      <label class="flex min-h-8 items-start gap-3 roomy:min-h-touch" for="{uid}-shown-{shown}">
        <input
          id="{uid}-shown-{shown}"
          type="radio"
          name="{uid}-shown"
          value={shown}
          bind:group={draft.nameShown}
          disabled={working}
          onchange={edited}
          aria-describedby={problems.nameShown ? `${uid}-shown-problem` : undefined}
          data-testid="name-shown-{shown}"
          class="mt-1 size-4 accent-primary"
        />
        <span class="grid min-w-0">
          <span>{NAME_SHOWN_LABELS[shown]()}</span>
          <span class="truncate text-xs text-muted-foreground" translate="no">
            {gives || m.account_name_shown_empty()}
          </span>
        </span>
      </label>
    {/each}
    {#if problems.nameShown}
      <p
        id="{uid}-shown-problem"
        class="text-sm text-destructive"
        role="alert"
        data-testid="problem-nameShown"
      >
        {problems.nameShown}
      </p>
    {/if}
  </fieldset>

  <div class="grid gap-1.5">
    <Label for="{uid}-title" class="font-medium">{m.account_name_title()}</Label>
    <select
      id="{uid}-title"
      class="h-8 max-w-full rounded-md border border-input-border bg-background px-2 roomy:h-touch"
      disabled={working}
      value={draft.title ?? ''}
      aria-invalid={problems.title ? 'true' : undefined}
      aria-describedby={problems.title ? `${uid}-title-problem` : undefined}
      data-testid="name-title"
      onchange={(event) => {
        const code = event.currentTarget.value;
        draft.title = TITLES.find((title) => title === code) ?? null;
        edited();
      }}
    >
      <option value="">{m.account_name_title_none()}</option>
      {#each TITLES as title (title)}
        <option value={title}>{TITLE_LABELS[title]()}</option>
      {/each}
    </select>
    {#if problems.title}
      <p
        id="{uid}-title-problem"
        class="text-sm text-destructive"
        role="alert"
        data-testid="problem-title"
      >
        {problems.title}
      </p>
    {/if}
  </div>

  <CredentialPicker
    bind:value={draft.credentials}
    disabled={working}
    problem={problems.credentials}
    onchange={edited}
  />

  <div class="flex flex-wrap items-center gap-2">
    <Button
      type="submit"
      variant="outline"
      class="roomy:min-h-touch"
      disabled={working || !changed}
    >
      {m.account_display_name_save()}
    </Button>
    <!-- Said once it is saved; the preview changes with every key and is read where it is. -->
    <p class="min-w-0 text-xs text-muted-foreground" data-testid="name-preview">
      {#if !saved}{m.account_name_preview({ name: preview })}{/if}
      <span aria-live="polite">{saved ? m.account_display_name_saved() : ''}</span>
    </p>
  </div>
</form>
