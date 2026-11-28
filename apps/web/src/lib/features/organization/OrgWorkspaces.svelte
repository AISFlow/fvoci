<!--
  The workspaces of the organization, for its administrators: making one,
  renaming one, joining one as its administrator (which its members read in
  its records) and leaving it, putting one in the trash (asked first; the
  default workspace has no such button) and bringing one back. Nothing of
  what is in a workspace is shown; a workspace the reader is in is opened
  from here.
-->
<script lang="ts">
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { OrgAdmin } from '$lib/state/org-admin.svelte.ts';
  import type { OrgWorkspace } from '$lib/state/organization.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

  import NameDialog from './NameDialog.svelte';

  interface Props {
    admin: OrgAdmin;
    workspaceHref: (workspaceId: string) => string;
  }

  let { admin, workspaceHref }: Props = $props();

  let newName = $state('');
  let renaming = $state.raw<OrgWorkspace | undefined>(undefined);
  let renameOpen = $state(false);
  let deleting = $state.raw<OrgWorkspace | undefined>(undefined);
  let deleteOpen = $state(false);

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (newName.trim() === '') return;
    if (await admin.createWorkspace(newName)) newName = '';
  }

  const nameOf = (workspace: OrgWorkspace): string => workspace.name || m.org_untitled_workspace();
</script>

<section class="flex flex-col gap-3" aria-labelledby="org-workspaces" data-testid="org-workspaces">
  <h2 id="org-workspaces" class="text-base font-semibold">{m.org_workspaces()}</h2>
  <form class="flex gap-2" onsubmit={(event) => void create(event)}>
    <Input
      bind:value={newName}
      aria-label={m.org_new_workspace_name()}
      placeholder={m.org_new_workspace_name()}
      maxlength={100}
      class="roomy:h-touch"
      data-testid="new-workspace-name"
    />
    <Button type="submit" class="shrink-0 roomy:min-h-touch" disabled={newName.trim() === ''}>
      {m.org_create_workspace()}
    </Button>
  </form>
  <ul class="flex flex-col gap-2">
    {#each admin.inUse as workspace (workspace.id)}
      <li
        class="flex flex-wrap items-center gap-2 rounded-md border px-3 py-2"
        data-testid="org-workspace"
        data-name={workspace.name}
      >
        <div class="min-w-0 flex-1 basis-56">
          <p class="truncate font-medium">
            {#if workspace.role}
              <a class="underline-offset-4 hover:underline" href={workspaceHref(workspace.id)}>
                {nameOf(workspace)}
              </a>
            {:else}
              {nameOf(workspace)}
            {/if}
          </p>
          <p class="text-xs text-muted-foreground">
            {m.org_member_count({ count: workspace.memberCount })}
            {#if workspace.isDefault}· {m.org_default_workspace()}{/if}
            {#if workspace.joinedAsOrgAdmin}· {m.org_joined_as_admin()}{:else if workspace.role}·
              {m.org_you_are_member()}{/if}
          </p>
        </div>
        <div class="flex flex-wrap gap-1">
          {#if workspace.joinedAsOrgAdmin}
            <Button
              size="sm"
              variant="outline"
              disabled={admin.busy}
              aria-label={m.org_leave_named({ name: nameOf(workspace) })}
              onclick={() => void admin.leaveWorkspace(workspace.id)}
            >
              {m.org_leave()}
            </Button>
          {:else if workspace.role !== 'admin'}
            <div class="flex max-w-64 flex-col items-start gap-1">
              <Button
                size="sm"
                variant="outline"
                disabled={admin.busy}
                aria-label={m.org_join_named({ name: nameOf(workspace) })}
                aria-describedby={`org-join-${workspace.id}`}
                onclick={() => void admin.joinWorkspace(workspace.id)}
              >
                {m.org_join()}
              </Button>
              <p id={`org-join-${workspace.id}`} class="text-xs text-muted-foreground">
                {m.org_join_hint()}
              </p>
            </div>
          {/if}
          <Button
            size="sm"
            variant="ghost"
            disabled={admin.busy}
            aria-label={m.org_rename_named({ name: nameOf(workspace) })}
            onclick={() => {
              renaming = workspace;
              renameOpen = true;
            }}
          >
            {m.org_rename()}
          </Button>
          {#if !workspace.isDefault}
            <Button
              size="sm"
              variant="ghost"
              class="text-destructive"
              disabled={admin.busy}
              aria-label={m.org_delete_named({ name: nameOf(workspace) })}
              onclick={() => {
                deleting = workspace;
                deleteOpen = true;
              }}
            >
              {m.org_delete()}
            </Button>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
  {#if admin.inTheTrash.length > 0}
    <h3 class="text-sm font-semibold text-muted-foreground">{m.org_trash()}</h3>
    <ul class="flex flex-col gap-2" data-testid="org-trash">
      {#each admin.inTheTrash as workspace (workspace.id)}
        <li class="flex items-center gap-2 rounded-md border border-dashed px-3 py-2">
          <span class="min-w-0 flex-1 truncate text-muted-foreground">{nameOf(workspace)}</span>
          <Button
            size="sm"
            variant="outline"
            disabled={admin.busy}
            aria-label={m.org_restore_named({ name: nameOf(workspace) })}
            onclick={() => void admin.restoreWorkspace(workspace.id)}
          >
            {m.org_restore()}
          </Button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<NameDialog
  bind:open={renameOpen}
  title={m.org_rename_workspace()}
  initial={renaming?.name ?? ''}
  onSubmit={(name: string) => {
    if (renaming && name !== renaming.name) void admin.renameWorkspace(renaming.id, name);
  }}
/>

<ConfirmDialog
  bind:open={deleteOpen}
  title={m.org_delete_workspace()}
  description={deleting ? m.org_delete_workspace_info({ name: nameOf(deleting) }) : undefined}
  confirmLabel={m.org_delete()}
  destructive
  onConfirm={() => {
    if (deleting) void admin.deleteWorkspace(deleting.id);
  }}
/>
