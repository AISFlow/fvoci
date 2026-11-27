<!--
  The layout readout: the width of the window, the page (the window less the
  edges the device keeps) and its stage, and the width of the pane the board
  is drawn in. For whoever works on the layout; off unless asked for
  (dev.svelte.ts).

  It sits in the bottom right corner (the left one has the reader's account),
  clear of the device's edges, over everything, and takes no presses:
  whatever is under it works as if it were not there. It is not read out: it
  describes the screen, not the content.
-->
<script lang="ts">
  import { innerHeight, innerWidth } from 'svelte/reactivity/window';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { page, pane, type Stage } from '$lib/layout/room.svelte.ts';

  const STAGE_LABELS: Record<Stage, () => string> = {
    compact: m.shell_stage_compact,
    medium: m.shell_stage_medium,
    expanded: m.shell_stage_expanded,
  };

  const round = (value: number | undefined): number => Math.round(value ?? 0);
</script>

<div
  data-testid="dev-readout"
  data-stage={page.stage}
  data-window={round(innerWidth.current)}
  data-page={round(page.width)}
  data-pane={round(pane.width)}
  aria-hidden="true"
  class="pointer-events-none fixed right-[calc(var(--safe-right)+0.5rem)] bottom-[calc(var(--safe-bottom)+var(--keyboard)+0.5rem)] z-[80] rounded-md bg-foreground/80 px-2 py-1 font-mono text-xs leading-tight text-background shadow-md"
>
  <div>
    {m.shell_dev_window()}
    {round(innerWidth.current)}×{round(innerHeight.current)}
  </div>
  <div>
    {m.shell_dev_page()}
    {round(page.width)} · {STAGE_LABELS[page.stage]()}
  </div>
  <div>{m.shell_dev_pane()} {round(pane.width)}</div>
</div>
