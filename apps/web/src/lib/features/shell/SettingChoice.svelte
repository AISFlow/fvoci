<!--
  One setting with a few answers, all of them in view: a row of buttons of
  which one is pressed in (a radio group). The arrow keys move the choice,
  as in any radio group, and Tab reaches the group once.

  An answer can be shown without being offered (`disabled`, with a `note`
  that says why): a place kept for something that is not built yet.
-->
<script lang="ts" generics="Value extends string">
  interface Option {
    value: Value;
    label: string;
    disabled?: boolean;
    /** A few words under the label, such as "Coming soon". */
    note?: string;
  }

  interface Props {
    /** Names the setting in the markup, for tests: `data-setting`. */
    name: string;
    label: string;
    value: Value;
    options: readonly Option[];
    onChange: (value: Value) => void;
  }

  let { name, label, value, options, onChange }: Props = $props();

  const uid = $props.id();
  let group = $state<HTMLElement | null>(null);

  /** The one answer Tab stops at: the chosen one, or the first that can be chosen. */
  const tabStop = $derived(
    options.find((option) => option.value === value && !option.disabled) ??
      options.find((option) => !option.disabled),
  );

  /** The next answer that can be chosen, going `step` from `from`, around the ends. */
  function neighbour(from: number, step: number): Option | undefined {
    for (let moved = 1; moved <= options.length; moved += 1) {
      const option = options[(from + step * moved + options.length * moved) % options.length];
      if (option && !option.disabled) return option;
    }
    return undefined;
  }

  function onkeydown(event: KeyboardEvent, at: number): void {
    const step =
      event.key === 'ArrowRight' || event.key === 'ArrowDown'
        ? 1
        : event.key === 'ArrowLeft' || event.key === 'ArrowUp'
          ? -1
          : 0;
    if (step === 0) return;
    event.preventDefault();
    const next = neighbour(at, step);
    if (!next) return;
    if (next.value !== value) onChange(next.value);
    group?.querySelector<HTMLElement>(`[data-value="${CSS.escape(next.value)}"]`)?.focus();
  }
</script>

<div class="grid gap-1.5" data-setting={name}>
  <span id="{uid}-label" class="font-medium">{label}</span>
  <div
    bind:this={group}
    role="radiogroup"
    aria-labelledby="{uid}-label"
    class="flex flex-wrap gap-1 rounded-lg bg-muted p-1"
  >
    {#each options as option, at (option.value)}
      {@const chosen = option.value === value}
      <button
        type="button"
        role="radio"
        aria-checked={chosen}
        data-value={option.value}
        disabled={option.disabled}
        tabindex={option === tabStop ? 0 : -1}
        class={[
          'flex min-h-8 min-w-20 flex-1 flex-col items-center justify-center rounded-md px-2 py-1 text-center roomy:min-h-touch',
          chosen
            ? 'bg-background font-medium text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground',
          option.disabled && 'cursor-not-allowed opacity-60 hover:text-muted-foreground',
        ]}
        onclick={() => {
          if (!chosen && !option.disabled) onChange(option.value);
        }}
        onkeydown={(event) => {
          onkeydown(event, at);
        }}
      >
        <span>{option.label}</span>
        {#if option.note}<span class="text-xs">{option.note}</span>{/if}
      </button>
    {/each}
  </div>
</div>
