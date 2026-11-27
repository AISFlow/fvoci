<!--
  Timer start / active-stop control placed in card details and course units.
  Starts with one click using the default kind, or lets the user pick a
  specific kind (`lecture` / `study` / `work` / `other`) via the menu.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SquareIcon from '@lucide/svelte/icons/square';

  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { formatTimerClock, TIME_ENTRY_KINDS, type TimeEntryKind } from '$lib/logic/time.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { kindLabel } from './time-labels.ts';

  interface Props {
    running: boolean;
    elapsedMs: number;
    busy: boolean;
    defaultKind: TimeEntryKind;
    onStart: (kind: TimeEntryKind) => void;
    onStop: () => void;
  }

  let { running, elapsedMs, busy, defaultKind, onStart, onStop }: Props = $props();

  const clock = $derived(formatTimerClock(elapsedMs));

  const kindActions = $derived<MenuAction[]>(
    TIME_ENTRY_KINDS.map((kind) => ({
      id: `start-${kind}`,
      label: m.time_start_timer_kind({ kind: kindLabel(kind) }),
      disabled: busy,
      run: () => {
        onStart(kind);
      },
    })),
  );
</script>

{#if running}
  <Button
    variant="outline"
    size="sm"
    disabled={busy}
    data-testid="target-timer-stop"
    aria-label={m.time_stop_timer()}
    class="h-8 gap-1.5 border-destructive/40 bg-destructive/10 px-2.5 text-xs text-destructive hover:bg-destructive/20 hover:text-destructive roomy:min-h-touch"
    onclick={onStop}
  >
    <span class="relative flex size-2 shrink-0" aria-hidden="true">
      <span
        class="absolute inline-flex size-full animate-ping rounded-full bg-destructive opacity-75 motion-reduce:hidden"
      ></span>
      <span class="relative inline-flex size-2 rounded-full bg-destructive"></span>
    </span>
    <span class="font-mono font-semibold tabular-nums" data-testid="target-timer-elapsed">
      {clock}
    </span>
    <SquareIcon class="size-3 fill-current" aria-hidden="true" />
    <span class="hidden sm:inline">{m.time_stop_short()}</span>
  </Button>
{:else}
  <div class="inline-flex shrink-0 items-center">
    <Button
      variant="outline"
      size="sm"
      disabled={busy}
      aria-label={m.time_start_timer()}
      data-testid="target-timer-start"
      class="h-8 gap-1.5 rounded-r-none border-r-0 px-2 text-xs sm:px-2.5 roomy:min-h-touch"
      onclick={() => {
        onStart(defaultKind);
      }}
    >
      <PlayIcon class="size-3.5" aria-hidden="true" />
      <span class="hidden sm:inline">{m.time_start_timer()}</span>
    </Button>
    <ActionMenu title={m.time_choose_kind()} actions={kindActions}>
      {#snippet trigger(props: Record<string, unknown>)}
        <Button
          {...props}
          variant="outline"
          size="sm"
          disabled={busy}
          data-testid="target-timer-kind"
          aria-label={m.time_choose_kind()}
          class="h-8 rounded-l-none px-1.5 roomy:min-h-touch"
        >
          <ChevronDownIcon class="size-3.5" aria-hidden="true" />
        </Button>
      {/snippet}
    </ActionMenu>
  </div>
{/if}
