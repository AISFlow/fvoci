<!--
  The notes of the day open on a diary's page, at the head of its entry: how
  the day was (one of five faces, each named, pressed again for none), its
  weather and its place, typed by hand. Nothing is looked up: no weather
  service is asked, and no place is asked of the device.

  What is marked is the day's at once (`EntryStore.mark`), a mood sent as it
  is pressed, what is typed once the typing pauses or the field is left; a day
  without an entry has one made for it. A syllable an input method is still
  composing is not marked before it is done.
-->
<script lang="ts">
  import CloudSunIcon from '@lucide/svelte/icons/cloud-sun';
  import MapPinIcon from '@lucide/svelte/icons/map-pin';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { MOODS, NOTE_MAX_CHARS, type EntryStore } from '$lib/state/index.ts';
  import { Input } from '$lib/ui/input/index.js';

  import { MOOD_FACES } from './moods.ts';

  interface Props {
    entry: EntryStore;
    /** The day written whole, for what the notes are called. */
    label: string;
  }

  let { entry, label }: Props = $props();

  const notes = $derived(entry.notes);
  const fields = [
    { field: 'weather', icon: CloudSunIcon, name: m.diary_weather },
    { field: 'place', icon: MapPinIcon, name: m.diary_place },
  ] as const;

  function typed(field: 'weather' | 'place', event: Event & { currentTarget: HTMLInputElement }) {
    if (event instanceof InputEvent && event.isComposing) return;
    entry.mark({ [field]: event.currentTarget.value }, 'later');
  }
</script>

<div
  role="group"
  aria-label={m.diary_notes_of({ date: label })}
  class="flex flex-wrap items-center gap-x-3 gap-y-2"
  data-testid="entry-notes"
>
  <div role="group" aria-label={m.diary_mood()} class="flex items-center gap-1">
    {#each MOODS as mood (mood)}
      {@const face = MOOD_FACES[mood]}
      {@const chosen = notes.mood === mood}
      <button
        type="button"
        class={[
          'grid size-8 place-items-center rounded-full outline-none hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/50 roomy:size-touch',
          chosen ? 'bg-accent text-foreground ring-1 ring-foreground' : 'text-muted-foreground',
        ]}
        aria-label={face.name()}
        aria-pressed={chosen}
        title={face.name()}
        data-testid="mood-{mood}"
        onclick={() => {
          entry.mark({ mood: chosen ? null : mood });
        }}
      >
        <face.icon class="size-5" />
      </button>
    {/each}
    {#if notes.mood}
      <span class="ps-1 text-sm" data-testid="mood-name">{MOOD_FACES[notes.mood].name()}</span>
    {/if}
  </div>
  <div class="flex min-w-0 flex-[1_1_16rem] gap-2">
    {#each fields as { field, icon: Icon, name } (field)}
      <label class="flex min-w-0 flex-1 items-center gap-1.5">
        <Icon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span class="sr-only">{name()}</span>
        <Input
          value={notes[field]}
          placeholder={name()}
          maxlength={NOTE_MAX_CHARS}
          autocomplete="off"
          class="h-8 roomy:h-touch"
          data-testid="entry-{field}"
          oninput={(event: Event & { currentTarget: HTMLInputElement }) => {
            typed(field, event);
          }}
          oncompositionend={(event: CompositionEvent & { currentTarget: HTMLInputElement }) => {
            entry.mark({ [field]: event.currentTarget.value }, 'later');
          }}
          onblur={() => {
            entry.sendNotes();
          }}
        />
      </label>
    {/each}
  </div>
</div>
