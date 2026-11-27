<!--
  Modal dialog wrapping `TimeEntryForm` for manually logging or editing a time entry.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CreateTimeEntryRequest, TimeEntry, TimeEntryPatch } from '$lib/state/index.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import TimeEntryForm from './TimeEntryForm.svelte';

  interface Props {
    open?: boolean;
    mode: 'create' | 'edit';
    entry?: TimeEntry | undefined;
    timeZone: string;
    onCreate: (request: CreateTimeEntryRequest) => Promise<void>;
    onPatch: (entryId: string, patch: TimeEntryPatch) => Promise<void>;
  }

  let { open = $bindable(false), mode, entry, timeZone, onCreate, onPatch }: Props = $props();
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] max-w-lg overflow-y-auto"
    data-testid="time-entry-dialog"
  >
    <Dialog.Header>
      <Dialog.Title>
        {mode === 'create' ? m.time_dialog_add_title() : m.time_dialog_edit_title()}
      </Dialog.Title>
    </Dialog.Header>
    {#if open}
      <TimeEntryForm
        {mode}
        {entry}
        {timeZone}
        {onCreate}
        {onPatch}
        onCancel={() => {
          open = false;
        }}
        onSaved={() => {
          open = false;
        }}
      />
    {/if}
  </Dialog.Content>
</Dialog.Root>
