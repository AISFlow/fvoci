<!--
  The head of the sidebar: the name of the workspace shown, which opens the
  list of the reader's workspaces to go to. The reader's own space comes first,
  as "My space" with a lock (nobody else is in it), then each organization's
  workspaces together: one that is the reader's only workspace of its
  organization goes by the organization's name, those of an organization of
  several are grouped under its name (`space-names.ts`). Where the reader may
  start an organization (`onCreateOrganization`), the list ends with that.
-->
<script lang="ts">
  import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
  import LockIcon from '@lucide/svelte/icons/lock';
  import LockKeyholeIcon from '@lucide/svelte/icons/lock-keyhole';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import UsersRoundIcon from '@lucide/svelte/icons/users-round';

  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isOwnSpace, type WorkspaceInfo } from '$lib/state/index.ts';

  import NewOrganizationDialog from './NewOrganizationDialog.svelte';
  import { spaceLines, spaceName, type OrgName, type SpaceLine } from './space-names.ts';

  interface Props {
    workspaces: readonly WorkspaceInfo[];
    /** The workspace shown. */
    current: string | undefined;
    /** The names of the organizations, which group the workspaces. */
    orgName: OrgName;
    onSwitch: (workspaceId: string) => void;
    /** Starts an organization of that name and goes to it; a refusal rejects. Left out where none can be started. */
    onCreateOrganization?: ((name: string) => Promise<void>) | undefined;
  }

  let { workspaces, current, orgName, onSwitch, onCreateOrganization }: Props = $props();

  let creating = $state(false);

  const shown = $derived(workspaces.find((workspace) => workspace.id === current));
  const shownName = $derived(spaceName(shown, workspaces, orgName));

  /** What a line says beyond its label: its group, how a locked own space opens, and which one is shown. */
  function ariaLabelOf({ workspace, label, fullName }: SpaceLine): { ariaLabel?: string } {
    const named = workspace.passwordRequired ? m.own_space_locked_label() : fullName;
    if (workspace.id === current) return { ariaLabel: m.shell_workspace_current({ name: named }) };
    return named === label ? {} : { ariaLabel: named };
  }

  const actions = $derived<MenuAction[]>([
    ...spaceLines(workspaces, orgName).map((line) => ({
      id: line.workspace.id,
      label: line.label,
      // The own space of an account with a password, to a session not made with it: it says so.
      icon: line.workspace.passwordRequired
        ? LockKeyholeIcon
        : isOwnSpace(line.workspace)
          ? LockIcon
          : UsersRoundIcon,
      checked: line.workspace.id === current,
      ...(line.heading === undefined ? {} : { heading: line.heading }),
      ...(line.separated ? { separated: true } : {}),
      ...ariaLabelOf(line),
      run: () => {
        if (line.workspace.id !== current) onSwitch(line.workspace.id);
      },
    })),
    ...(onCreateOrganization
      ? [
          {
            id: 'new-organization',
            label: m.org_new(),
            icon: PlusIcon,
            separated: workspaces.length > 0,
            run: () => {
              creating = true;
            },
          },
        ]
      : []),
  ]);
</script>

<ActionMenu title={m.shell_switch_workspace()} {actions} align="start">
  {#snippet trigger(props: Record<string, unknown>)}
    <button
      {...props}
      type="button"
      data-testid="workspace-switcher"
      data-own-space={isOwnSpace(shown) ? '' : undefined}
      aria-label={m.shell_switch_workspace_from({ name: shownName })}
      class="flex min-h-8 min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
    >
      {#if isOwnSpace(shown)}
        <LockIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      {/if}
      <span class="min-w-0 flex-1 truncate text-lg font-semibold tracking-tight">
        {shownName}
      </span>
      <ChevronsUpDownIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    </button>
  {/snippet}
</ActionMenu>

{#if onCreateOrganization}
  <NewOrganizationDialog bind:open={creating} onCreate={onCreateOrganization} />
{/if}
