<!--
  One picked card row inside the open `RelationProperty` picker: shows its
  title, optional loop marker, inline `kind` (`FS`/`SS`/`FF`/`SF`) selector,
  `lagDays` input (`-3650..=3650`), and remove button.
-->
<script lang="ts">
  import RepeatIcon from '@lucide/svelte/icons/repeat';
  import XIcon from '@lucide/svelte/icons/x';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    parseLagDaysInput,
    RELATION_KINDS,
    RELATION_LAG_MAX,
    RELATION_LAG_MIN,
    type RelationKind,
    type RelationLink,
  } from '$lib/logic/relations.ts';

  interface Props {
    link: RelationLink;
    title: string;
    inLoop: boolean;
    onUpdate: (patch: { kind?: RelationKind; lagDays?: number }) => void;
    onRemove: () => void;
  }

  let { link, title, inLoop, onUpdate, onRemove }: Props = $props();

  const uid = $props.id();
  let lagText = $derived(String(link.lagDays));
  const lagValid = $derived(parseLagDaysInput(lagText) !== undefined);
  const rangeHint = $derived(
    m.gantt_relation_lag_range({ min: RELATION_LAG_MIN, max: RELATION_LAG_MAX }),
  );

  function commitLag(): void {
    const parsed = parseLagDaysInput(lagText);
    if (parsed === undefined) {
      lagText = String(link.lagDays);
      return;
    }
    lagText = String(parsed);
    if (parsed !== link.lagDays) onUpdate({ lagDays: parsed });
  }
</script>

<li
  data-slot="relation-picked-item"
  data-card-id={link.card}
  class="flex min-w-0 items-center gap-1 rounded-md bg-muted px-1.5 py-1 text-sm text-foreground"
>
  <span class="min-w-0 flex-1 truncate">{title}</span>
  {#if inLoop}
    <RepeatIcon
      class="size-3.5 shrink-0 text-destructive"
      aria-hidden="true"
      data-slot="relation-loop"
    />
    <span class="sr-only">{m.property_relation_in_loop()}</span>
  {/if}
  <select
    data-slot="relation-kind-select"
    aria-label={m.property_relation_kind_for({ title })}
    value={link.kind}
    class="h-6 rounded-sm border border-input bg-background px-1 text-base font-medium text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring md:text-xs"
    onchange={(event) => {
      onUpdate({ kind: event.currentTarget.value as RelationKind });
    }}
  >
    {#each RELATION_KINDS as kind (kind)}
      <option value={kind}>{kind.toUpperCase()}</option>
    {/each}
  </select>
  <input
    data-slot="relation-lag-input"
    type="text"
    aria-label={m.property_relation_lag_for({ title })}
    aria-invalid={lagValid ? undefined : 'true'}
    aria-describedby={lagValid ? undefined : `${uid}-lag-error`}
    title={lagValid ? undefined : rangeHint}
    bind:value={lagText}
    class="h-6 w-14 rounded-sm border border-input bg-background px-1 text-right text-base text-foreground tabular-nums outline-none focus-visible:ring-2 focus-visible:ring-ring aria-invalid:border-destructive md:text-xs"
    onchange={commitLag}
    onblur={commitLag}
    onkeydown={(event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        commitLag();
      } else if (event.key === 'Escape') {
        lagText = String(link.lagDays);
      }
    }}
  />
  {#if !lagValid}
    <span id="{uid}-lag-error" role="alert" class="sr-only">{rangeHint}</span>
  {/if}
  <button
    type="button"
    class="relative inline-flex size-5 shrink-0 items-center justify-center rounded-sm group-data-[presentation=sheet]/surface:size-8 group-data-[presentation=sheet]/surface:after:absolute group-data-[presentation=sheet]/surface:after:-inset-1.5 hover:bg-foreground/10"
    aria-label={m.property_option_remove({ name: title })}
    onclick={onRemove}
  >
    <XIcon class="size-3.5" aria-hidden="true" />
  </button>
</li>
