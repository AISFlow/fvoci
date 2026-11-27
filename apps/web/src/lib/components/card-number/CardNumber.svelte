<!--
  A card's number on its board: shown as `#12`, and said as words (“number
  12”), as a screen reader would not say the sign. Nothing for a card that
  has none (a template). It does not shrink: a title next to it is cut short
  first.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { cardNumberText } from '$lib/logic/card-numbers.ts';

  interface Props {
    number: number | undefined;
    class?: string;
  }

  let { number, class: className }: Props = $props();
</script>

{#if number !== undefined}
  <span
    data-slot="card-number"
    data-number={number}
    class={['shrink-0 font-normal text-muted-foreground tabular-nums', className]}
  >
    <span aria-hidden="true">{cardNumberText(number)}</span>
    <!-- Apart from the words around it, which a screen reader would run into it. -->
    <span class="sr-only">{` ${m.card_number_said({ number })} `}</span>
  </span>
{/if}
