<!--
  The body of the version being read, in the dialog of the versions
  (BodyVersions.svelte): by default what it changed from the version before
  it, or compared with what the reader chooses (version-compare.svelte.ts):
  the body now, or another version kept; or as it is. A line says from what
  to what it is compared, so that what is added and what is removed read the
  right way round whichever is the newer.
-->
<script lang="ts">
  import BodyView from '$lib/editor/BodyView.svelte';
  import type { ReferenceDrawing } from '$lib/editor/reference-chip.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BodyVersion } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import type { VersionsReading } from './body-versions.svelte.ts';
  import VersionDiff from './VersionDiff.svelte';

  interface Props {
    reading: VersionsReading;
    /** When a version was kept, how, and by whom, as the list says it. */
    whenKept: (version: BodyVersion) => string;
    causeOf: (version: BodyVersion) => string;
    whoWrote: (version: BodyVersion) => string;
    resolveUser: (userId: string) => string;
    references?: ReferenceDrawing;
    /** What is said of a version with no body, and of one that cannot be shown. */
    words: { noBody: () => string; invalid: () => string };
  }

  let { reading, whenKept, causeOf, whoWrote, resolveUser, references, words }: Props = $props();

  const compare = $derived(reading.compare);
  const chosen = $derived(reading.chosen);
  const target = $derived(compare.target);
  /** The other versions to compare with: all kept but the one read. */
  const others = $derived(reading.versions.filter((one) => one.version !== chosen?.version));
  /** What the choice says, also while the version is shown as it is: what it is compared with then. */
  const choice = $derived.by(() => {
    const chosenTarget = compare.chosenTarget;
    if (chosenTarget?.kind === 'version') return String(chosenTarget.version.version);
    return chosenTarget?.kind === 'current' ? 'current' : 'previous';
  });
  const direction = $derived.by((): string | undefined => {
    if (!chosen || !target) return undefined;
    const to = whenKept(chosen);
    switch (target.kind) {
      case 'previous':
        return m.card_version_compare_from_previous({ from: whenKept(target.version), to });
      case 'current':
        return m.card_version_compare_from_current({ to });
      case 'version':
        return target.version.version < chosen.version
          ? m.card_version_compare_from_older({ from: whenKept(target.version), to })
          : m.card_version_compare_from_newer({ from: whenKept(target.version), to });
      case 'oldest':
        return m.card_version_compare_oldest();
      default:
        return undefined;
    }
  });
  /** Shown as it is: so chosen, the oldest, or a body that cannot be compared. */
  const asIs = $derived(
    target?.kind === 'whole' || target?.kind === 'oldest' || compare.status === 'invalid',
  );

  function optionLabel(version: BodyVersion): string {
    const when = whenKept(version);
    const cause = causeOf(version);
    return version.editors.length > 0
      ? m.card_version_compare_option_by({ when, cause, who: whoWrote(version) })
      : m.card_version_compare_option({ when, cause });
  }

  function chose(value: string): void {
    compare.compareWith(value === 'previous' || value === 'current' ? value : Number(value));
  }
</script>

<div class="flex flex-wrap items-center gap-2 border-b px-4 py-2">
  <label class="flex min-w-0 flex-1 items-center gap-2 text-xs text-muted-foreground">
    <span class="shrink-0">{m.card_version_compare_with()}</span>
    <select
      class="h-8 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 text-base text-foreground sm:flex-initial md:text-sm roomy:h-touch"
      value={choice}
      data-testid="body-version-compare"
      onchange={(event) => {
        chose(event.currentTarget.value);
      }}
    >
      <option value="previous">{m.card_version_compare_previous()}</option>
      <option value="current">{m.card_version_compare_current()}</option>
      {#if others.length > 0}
        <optgroup label={m.card_version_compare_others()}>
          {#each others as version (version.version)}
            <option value={String(version.version)}>{optionLabel(version)}</option>
          {/each}
        </optgroup>
      {/if}
    </select>
  </label>
  <Button
    variant="outline"
    size="sm"
    class="roomy:min-h-touch"
    aria-pressed={compare.whole}
    data-testid="body-version-whole"
    onclick={() => {
      compare.showWhole(!compare.whole);
    }}
  >
    {m.card_version_whole()}
  </Button>
</div>
<div class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 py-3">
  {#if direction}
    <p class="mb-3 text-xs text-muted-foreground" data-testid="body-version-direction">
      {direction}
    </p>
  {/if}
  {#if asIs}
    <BodyView doc={reading.content?.body ?? null} {resolveUser} {references} class="text-base">
      {#snippet empty()}
        <p class="text-muted-foreground">{words.noBody()}</p>
      {/snippet}
      {#snippet invalid()}
        <p class="text-muted-foreground" role="alert">{words.invalid()}</p>
      {/snippet}
    </BodyView>
  {:else if compare.status === 'ready' && compare.diff}
    {#key compare.diff}
      <VersionDiff diff={compare.diff} {resolveUser} {references} />
    {/key}
  {:else if compare.status === 'failed'}
    <p role="alert" data-testid="body-version-compare-failed">{m.card_version_compare_failed()}</p>
  {:else}
    <p class="text-muted-foreground" role="status">{m.body_diff_comparing()}</p>
  {/if}
</div>
