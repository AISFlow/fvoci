<!--
  Global top-bar pill for the single running personal timer.
  Clicking the pill navigates to the tracked card/unit (or My time when targetless);
  the stop button stops the timer immediately. On a phone's narrow pane it is
  the dot, the clock and the stop button alone: the target is in the link's
  name, and shows from a pane of `medium` on.
-->
<script lang="ts">
  import SquareIcon from '@lucide/svelte/icons/square';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { formatTimerClock } from '$lib/logic/time.ts';
  import type { TimeEntry } from '$lib/state/index.ts';

  import { KIND_BADGE_CLASS, kindLabel, pillTargetLabel } from './time-labels.ts';

  interface Props {
    entry: TimeEntry;
    elapsedMs: number;
    busy: boolean;
    href: string;
    onOpen: () => void;
    onStop: () => void;
  }

  let { entry, elapsedMs, busy, href, onOpen, onStop }: Props = $props();

  const label = $derived(pillTargetLabel(entry));
  const clock = $derived(formatTimerClock(elapsedMs));

  function handleOpen(event: MouseEvent): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpen();
  }
</script>

<div
  data-testid="active-timer-pill"
  class="flex max-w-44 shrink-0 items-center overflow-hidden rounded-full border border-destructive/40 bg-destructive/10 text-xs text-foreground @columns/pane:max-w-72"
>
  <a
    {href}
    data-testid="active-timer-link"
    aria-label={m.time_pill_open({ target: label })}
    title={label}
    class="flex min-w-0 items-center gap-1.5 py-1 pr-1.5 pl-2.5 transition-colors hover:bg-destructive/15 focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring roomy:min-h-touch"
    onclick={handleOpen}
  >
    <span class="relative flex size-2 shrink-0" aria-hidden="true">
      <span
        class="absolute inline-flex size-full animate-ping rounded-full bg-destructive opacity-75 motion-reduce:hidden"
      ></span>
      <span class="relative inline-flex size-2 rounded-full bg-destructive"></span>
    </span>
    <span
      class={[
        'hidden shrink-0 rounded-full border px-1.5 py-0 text-[0.6875rem] leading-4 font-medium @columns/pane:inline-block',
        KIND_BADGE_CLASS[entry.kind],
      ]}
      data-testid="active-timer-kind"
    >
      {kindLabel(entry.kind)}
    </span>
    <span
      class="hidden min-w-0 truncate font-medium @medium/pane:block"
      data-testid="active-timer-title"
    >
      {label}
    </span>
    <span
      class="shrink-0 font-mono text-[0.75rem] font-semibold text-destructive tabular-nums"
      data-testid="active-timer-elapsed"
    >
      {clock}
    </span>
  </a>
  <button
    type="button"
    data-testid="active-timer-stop"
    aria-label={m.time_stop_timer()}
    title={m.time_stop_timer()}
    disabled={busy}
    class="grid size-7 shrink-0 place-items-center border-l border-destructive/25 text-destructive transition-colors hover:bg-destructive/20 focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring disabled:opacity-50 roomy:size-touch"
    onclick={onStop}
  >
    <SquareIcon class="size-3 fill-current" aria-hidden="true" />
  </button>
</div>
