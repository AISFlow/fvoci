<!--
  The notes of a folded day of a diary's page, to read: its mood's face and
  name, its weather and its place, as far as it has them.
-->
<script lang="ts">
  import CloudSunIcon from '@lucide/svelte/icons/cloud-sun';
  import MapPinIcon from '@lucide/svelte/icons/map-pin';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { DayNotes } from '$lib/state/index.ts';

  import { MOOD_FACES } from './moods.ts';

  let { notes }: { notes: DayNotes } = $props();

  const face = $derived(notes.mood ? MOOD_FACES[notes.mood] : undefined);
</script>

{#if face !== undefined || notes.weather !== '' || notes.place !== ''}
  <p
    class="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground"
    data-testid="entry-notes-shown"
  >
    {#if face}
      <span class="flex items-center gap-1 text-foreground" data-testid="mood-shown">
        <face.icon class="size-4" />
        <span class="sr-only">{m.diary_mood()}:</span>
        {face.name()}
      </span>
    {/if}
    {#if notes.weather}
      <span class="flex min-w-0 items-center gap-1">
        <CloudSunIcon class="size-4 shrink-0" aria-hidden="true" />
        <span class="sr-only">{m.diary_weather()}:</span>
        <span class="truncate">{notes.weather}</span>
      </span>
    {/if}
    {#if notes.place}
      <span class="flex min-w-0 items-center gap-1">
        <MapPinIcon class="size-4 shrink-0" aria-hidden="true" />
        <span class="sr-only">{m.diary_place()}:</span>
        <span class="truncate">{notes.place}</span>
      </span>
    {/if}
  </p>
{/if}
