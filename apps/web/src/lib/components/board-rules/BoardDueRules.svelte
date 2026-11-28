<!--
  A board's due date property and its rule of done: what the Today screen
  reads, and what lets the board's cards repeat. Drawn once the board has a
  date property or a property a rule of done can read; each choice is told
  at once (`onDueProperty`, `onDoneRule`). Who may change the board's
  properties is the caller's to know. In the view's properties, and under
  the choice of how a card repeats on a board that does not let it yet.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { doneChoices as doneChoicesOf, doneChoiceValue } from '$lib/logic/done-rule.ts';
  import type { DoneRule, PropertyTemplate } from '$lib/logic/wire.ts';

  interface Props {
    /** The board's card properties. */
    properties: readonly PropertyTemplate[];
    /** The date property it names as its due date; none falls back to its first. */
    dueDatePropertyId: string | null | undefined;
    doneWhen: DoneRule | null | undefined;
    onDueProperty: (propertyId: string | undefined) => void;
    onDoneRule: (rule: DoneRule | undefined) => void;
  }

  let { properties, dueDatePropertyId, doneWhen, onDueProperty, onDoneRule }: Props = $props();

  const dateProperties = $derived(properties.filter((prop) => prop.type === 'date'));
  const doneChoices = $derived(doneChoicesOf(properties));
  const selectedDoneValue = $derived(doneChoiceValue(doneWhen));
  const selectClass =
    'h-8 rounded-md border border-input-border bg-background px-2 text-base md:text-xs group-data-[presentation=sheet]/surface:h-touch';
</script>

{#if dateProperties.length > 0 || doneChoices.length > 0}
  <div class="grid gap-2 border-t p-2 text-xs" data-testid="board-today-rules">
    {#if dateProperties.length > 0}
      <label class="grid gap-1">
        <span class="font-medium text-muted-foreground">{m.view_due_date_property()}</span>
        <select
          class={selectClass}
          data-testid="board-due-date-property"
          value={dueDatePropertyId ?? ''}
          onchange={(e) => {
            onDueProperty(e.currentTarget.value || undefined);
          }}
        >
          <option value="">{m.view_due_date_auto()}</option>
          {#each dateProperties as prop (prop.id)}
            <option value={prop.id}>{prop.name}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if doneChoices.length > 0}
      <label class="grid gap-1">
        <span class="font-medium text-muted-foreground">{m.view_done_when()}</span>
        <select
          class={selectClass}
          data-testid="board-done-when"
          value={selectedDoneValue}
          onchange={(e) => {
            const value = e.currentTarget.value;
            onDoneRule(doneChoices.find((item) => item.value === value)?.rule);
          }}
        >
          <option value="">{m.view_done_none()}</option>
          {#each doneChoices as choice (choice.value)}
            <option value={choice.value}>{choice.label}</option>
          {/each}
        </select>
      </label>
    {/if}
  </div>
{/if}
