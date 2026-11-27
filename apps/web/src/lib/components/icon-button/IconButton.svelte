<!--
  A button that is only an icon. 32 pixels for a mouse; in the roomy density
  (a finger, or the reader's choice) it grows to the smallest side a finger
  can hit (`--spacing-touch`).

  `label` is not optional: a tooltip does not show without hover, so the name
  for a screen reader is the only name the button has.
-->
<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements';

  import { Button } from '$lib/ui/button/index.js';

  type Props = HTMLButtonAttributes & {
    /** What the button does, for a screen reader. */
    label: string;
    ref?: HTMLElement | null;
  };

  let { label, ref = $bindable(null), class: className, children, ...rest }: Props = $props();

  // The generated button takes the attributes of a button and of a link at once;
  // those of a button alone do not fit its type.
  const passed = $derived<Record<string, unknown>>(rest);
</script>

<Button
  bind:ref
  variant="ghost"
  size="icon"
  aria-label={label}
  title={label}
  class={['roomy:size-touch', className]}
  {...passed}
>
  {@render children?.()}
</Button>
