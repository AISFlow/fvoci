<!--
  The records of the workspace, which every member who works in it reads:
  who joined it as the organization's administrator, left it or was taken
  out, and who gave a role on a board they could not read (to themselves or
  to somebody else). Read anew each time it opens. The back button closes it.
-->
<script lang="ts">
  import AuditList from '$lib/components/audit/AuditList.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import type { AuditRecord } from '$lib/state/organization.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  interface Props {
    open: boolean;
    workspaceId: string | undefined;
    locale: string;
    readRecords: (workspaceId: string) => Promise<AuditRecord[]>;
  }

  let { open = $bindable(), workspaceId, locale, readRecords }: Props = $props();

  let status = $state<'loading' | 'ready' | 'failed'>('loading');
  let records = $state.raw<AuditRecord[]>([]);

  // Every opening reads them anew. An answer that comes after the dialog closed, or
  // after it went to another workspace, is dropped.
  $effect(() => {
    if (!open || !workspaceId) return;
    let live = true;
    status = 'loading';
    readRecords(workspaceId).then(
      (read) => {
        if (!live) return;
        records = read;
        status = 'ready';
      },
      () => {
        if (live) status = 'failed';
      },
    );
    return () => {
      live = false;
    };
  });

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    data-testid="audit-dialog"
    class="max-h-[calc(85dvh-var(--keyboard))] overflow-y-auto"
  >
    <Dialog.Header>
      <Dialog.Title>{m.audit_title()}</Dialog.Title>
      <Dialog.Description>{m.audit_description()}</Dialog.Description>
    </Dialog.Header>
    {#if status === 'loading'}
      <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
    {:else if status === 'failed'}
      <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
    {:else}
      <AuditList {records} {locale} testId="workspace-records" />
    {/if}
  </Dialog.Content>
</Dialog.Root>
