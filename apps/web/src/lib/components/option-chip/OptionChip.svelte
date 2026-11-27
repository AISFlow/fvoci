<!--
  The label of a select option: its name on the fill of its colour. With
  `onRemove` it carries a button that takes the option out of a value.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { cn } from '$lib/utils.js';

  import { optionFill, type OptionColor } from './colors.ts';

  interface Props {
    name: string;
    /** A colour name such as `brown`. */
    color: OptionColor;
    onRemove?: () => void;
    class?: string;
  }

  let { name, color, onRemove, class: className }: Props = $props();
</script>

<span
  data-slot="option-chip"
  data-color={color}
  class={cn(
    'inline-flex max-w-full min-w-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-sm text-foreground',
    optionFill(color),
    className,
  )}
>
  <span class="truncate">{name}</span>
  {#if onRemove}
    <button
      type="button"
      class="relative -mr-1 inline-flex size-5 shrink-0 items-center justify-center rounded-sm group-data-[presentation=sheet]/surface:size-8 group-data-[presentation=sheet]/surface:after:absolute group-data-[presentation=sheet]/surface:after:-inset-1.5 hover:bg-foreground/10"
      aria-label={m.property_option_remove({ name })}
      onclick={(event) => {
        event.stopPropagation();
        onRemove();
      }}
    >
      <XIcon class="size-3.5" aria-hidden="true" />
    </button>
  {/if}
</span>
