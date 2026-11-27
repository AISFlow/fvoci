<!--
  The value of a person property: one person, or several. A person is picked
  from the list the caller passes, shown as "Display name (@login)" or by the
  login name alone; typing narrows the list by either name.

  A person is named from `named` too, the people known beyond those offered
  (an author who left). An identifier known to neither is shown as an unknown
  user: the value still says someone is there, and that someone can be taken out.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import PickList from '$lib/components/pick-list/PickList.svelte';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { searchedNames } from '$lib/logic/person-names.ts';
  import { isGone, userDisplayName } from '$lib/logic/users.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { cn } from '$lib/utils.js';

  import type { PropertyUser } from '../types.ts';

  interface Props {
    /** The name of the property. */
    name: string;
    /** The identifiers of the people in the value. */
    ids: readonly string[];
    multiple: boolean;
    users: readonly PropertyUser[];
    /** People who may be named though not offered (an author who left). */
    named?: readonly PropertyUser[];
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    presentation?: Presentation;
    startOpen?: boolean;
    /** One identifier (or `''`) for a single person, a list for several. */
    onChange?: (value: string | string[]) => void;
  }

  let {
    name,
    ids,
    multiple,
    users,
    named,
    editable,
    placeholder,
    presentation = 'popover',
    startOpen = false,
    onChange,
  }: Props = $props();

  // svelte-ignore state_referenced_locally (only where editing starts)
  let open = $state(startOpen);
  let search = $state('');

  const nameOf = (id: string): string => {
    const known = (candidate: PropertyUser): boolean => candidate.id === id;
    const user = users.find(known) ?? named?.find(known);
    if (!user) return m['centerPanel.unknown-user']();
    // What is left of a deleted account has no name: the card keeps who it was set to.
    return isGone(user) ? m.people_deleted_user() : userDisplayName(user);
  };

  /** Found by the login name, the name shown and every part of the name. */
  const searchTexts = (user: PropertyUser): string[] => [
    user.username,
    user.displayName,
    ...(user.name ? searchedNames(user.name) : []),
  ];

  function pick(user: PropertyUser): void {
    if (!multiple) {
      if (ids[0] !== user.id || ids.length !== 1) onChange?.(user.id);
      open = false;
      return;
    }
    onChange?.(ids.includes(user.id) ? ids.filter((id) => id !== user.id) : [...ids, user.id]);
  }

  function remove(id: string): void {
    onChange?.(multiple ? ids.filter((other) => other !== id) : '');
  }

  function clear(): void {
    onChange?.(multiple ? [] : '');
    if (!multiple) open = false;
  }

  const chipClass =
    'inline-flex max-w-full min-w-0 items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-sm text-foreground';
</script>

{#snippet people()}
  {#each ids as id (id)}
    <span data-slot="person-chip" class={chipClass}><span class="truncate">{nameOf(id)}</span></span
    >
  {/each}
  {#if ids.length === 0 && placeholder !== ''}
    <span data-slot="property-empty" class="text-muted-foreground">{placeholder}</span>
  {/if}
{/snippet}

{#if editable}
  <PickerSurface
    bind:open
    {presentation}
    title={name}
    onOpenChange={(next) => {
      if (!next) search = '';
    }}
  >
    {#snippet trigger({ props }: { props: Record<string, unknown> })}
      <button
        {...props}
        type="button"
        data-slot="property-person"
        class="flex min-h-8 w-full min-w-0 flex-wrap items-center gap-1 rounded-md px-2 py-1 text-left hover:bg-muted aria-expanded:bg-muted roomy:min-h-touch"
      >
        <span class="sr-only">{name}</span>
        {@render people()}
      </button>
    {/snippet}

    <div class="flex flex-col gap-1">
      {#if multiple && ids.length > 0}
        <ul class="m-0 flex list-none flex-wrap gap-1 p-1" aria-label={name}>
          {#each ids as id (id)}
            <li class={cn(chipClass, 'pr-0.5')}>
              <span class="truncate">{nameOf(id)}</span>
              <button
                type="button"
                class="relative inline-flex size-5 shrink-0 items-center justify-center rounded-sm group-data-[presentation=sheet]/surface:size-8 group-data-[presentation=sheet]/surface:after:absolute group-data-[presentation=sheet]/surface:after:-inset-1.5 hover:bg-foreground/10"
                aria-label={m.property_option_remove({ name: nameOf(id) })}
                onclick={() => {
                  remove(id);
                }}
              >
                <XIcon class="size-3.5" aria-hidden="true" />
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      <PickList
        bind:search
        items={users}
        textOf={searchTexts}
        selected={ids}
        {multiple}
        label={name}
        placeholder={m['ConfirmPerson.search']()}
        emptyText={m.property_no_matches()}
        onPick={pick}
        onBackspaceEmpty={() => {
          const last = ids.at(-1);
          if (multiple && last !== undefined) remove(last);
        }}
      >
        {#snippet row(user: PropertyUser)}
          <span class="truncate">{nameOf(user.id)}</span>
        {/snippet}
      </PickList>
      {#if ids.length > 0}
        <Button
          variant="ghost"
          class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
          onclick={clear}
        >
          {m.property_clear()}
        </Button>
      {/if}
    </div>
  </PickerSurface>
{:else}
  <span
    data-slot="property-person"
    class="flex min-h-8 min-w-0 flex-wrap items-center gap-1 px-2 py-1"
  >
    {@render people()}
  </span>
{/if}
