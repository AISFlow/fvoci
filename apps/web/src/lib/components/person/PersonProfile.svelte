<!--
  A person in a list of members: their name with title and credentials
  (`titledName`) and their login name, a button that opens a small card of
  their profile. The card tells the name shown, the parts of the name, the
  title, the credentials in the screen's words and the login name; Escape
  closes it and gives the focus back to the button.

  An account that was deleted (no login name) is its name alone, without a
  card: nothing of it is left to tell.
-->
<script lang="ts">
  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { fullName, latinName, type PersonName } from '$lib/logic/person-names.ts';
  import * as Popover from '$lib/ui/popover/index.js';

  import { CREDENTIAL_LABELS, TITLE_LABELS } from './name-labels.ts';
  import { titledName } from './titled-name.ts';

  interface Props {
    /** Whom the colour of the picture is picked by. */
    userId: string;
    username: string;
    /** The name the person is shown by; empty when that is the login name. */
    displayName: string;
    /** The parts of the name, the title and the credentials, where they were told. */
    name?: PersonName;
    /** The reader's language, for the picture's character. */
    locale: string;
    /** What a deleted account is called. */
    goneName?: string;
  }

  let { userId, username, displayName, name, locale, goneName = '' }: Props = $props();

  const uid = $props.id();
  const shown = $derived(displayName || username);
  const titled = $derived(titledName(shown, name));
  /** The rows of the card: those the person has filled. */
  const rows = $derived(
    name
      ? [
          { key: 'name', label: m.profile_card_name(), value: fullName(name) },
          { key: 'latin', label: m.profile_card_latin_name(), value: latinName(name) },
          {
            key: 'title',
            label: m.profile_card_title(),
            value: name.title ? TITLE_LABELS[name.title]() : '',
          },
          {
            key: 'credentials',
            label: m.profile_card_credentials(),
            value: name.credentials.map((credential) => CREDENTIAL_LABELS[credential]()).join(', '),
          },
        ].filter((row) => row.value !== '')
      : [],
  );
</script>

{#if username === ''}
  <span class="min-w-0 truncate text-muted-foreground">{goneName}</span>
{:else}
  <Popover.Root>
    <Popover.Trigger
      class="flex max-w-full min-w-0 items-baseline gap-1.5 rounded-sm text-start outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="person-profile"
      data-person={username}
    >
      <span class="min-w-0 truncate" translate="no">{titled}</span>
      {#if displayName}
        <span class="max-w-[50%] min-w-0 shrink-0 truncate text-muted-foreground" translate="no"
          >@{username}</span
        >
      {/if}
    </Popover.Trigger>
    <Popover.Content
      class="w-80 max-w-[calc(100vw-2rem)]"
      align="start"
      role="dialog"
      aria-labelledby="{uid}-name"
      data-testid="profile-card"
    >
      <div class="flex items-center gap-3">
        <Avatar name={shown} colorKey={userId} {locale} decorative class="size-10 text-base" />
        <div class="min-w-0">
          <p id="{uid}-name" class="font-medium break-words" translate="no">{titled}</p>
          <p class="break-all text-muted-foreground" translate="no">
            <span class="sr-only">{m.profile_card_username()}: </span>@{username}
          </p>
        </div>
      </div>
      {#if rows.length > 0}
        <dl class="grid gap-2">
          {#each rows as row (row.key)}
            <div>
              <dt class="text-xs text-muted-foreground">{row.label}</dt>
              <dd class="break-words" translate="no" data-row={row.key}>{row.value}</dd>
            </div>
          {/each}
        </dl>
      {/if}
    </Popover.Content>
  </Popover.Root>
{/if}
