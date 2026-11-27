<!--
  The units due today on the hub of today: each opened by its title, and put
  off on the reader's own plan by a day, three or a week at a press.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { TodayHubStore, TodayUnitEntry } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    store: TodayHubStore;
    spaceOf: (unit: TodayUnitEntry) => string;
    onOpen: (unit: TodayUnitEntry) => void;
  }

  let { store, spaceOf, onOpen }: Props = $props();

  const PUT_OFF_DAYS = [1, 3, 7] as const;
  const units = $derived(store.data?.units ?? []);
</script>

{#if units.length > 0}
  <section class="grid gap-2" data-testid="today-hub-units">
    <h3 class="text-sm font-semibold text-muted-foreground">{m.today_hub_learning()}</h3>
    <ul class="grid gap-1.5">
      {#each units as unit (unit.unitId)}
        {@const title = unit.title || m.course_unit_untitled()}
        <li class="flex flex-wrap items-center gap-1 rounded-md border pr-1">
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center justify-between gap-3 rounded-md px-3 py-2 text-left hover:bg-accent/50 roomy:min-h-touch"
            onclick={() => {
              onOpen(unit);
            }}
          >
            <span class="min-w-0 flex-1 truncate font-medium">{title}</span>
            <span class="shrink-0 text-xs text-muted-foreground">
              {spaceOf(unit)} · {unit.courseTitle || m.course_untitled()} · {unit.effectiveDate}
            </span>
          </button>
          <span class="flex gap-1" data-testid="today-hub-put-off">
            {#each PUT_OFF_DAYS as days (days)}
              <Button
                variant="ghost"
                size="sm"
                class="tabular-nums roomy:min-h-touch roomy:min-w-touch"
                aria-label={m.today_hub_put_off({ title, days })}
                onclick={() => void store.putOff(unit, days)}
              >
                +{days}
              </Button>
            {/each}
          </span>
        </li>
      {/each}
    </ul>
  </section>
{/if}
