<!--
  The groups of this workspace. Members read them; administrators manage them.
  Among the other parts of the people (`foldable`), it is folded until it has a group.
-->
<script lang="ts">
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { nameOfWorkspace } from '$lib/components/workspace-name.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { administers } from '$lib/state/people.ts';
  import {
    isBackendError,
    type People,
    type Workspace,
    type WorkspaceMember,
  } from '$lib/state/index.ts';
  import type { Team, Teams } from '$lib/state/team-types.ts';
  import { Button } from '$lib/ui/button/index.js';
  import FoldHeading from '$lib/components/fold/FoldHeading.svelte';
  import { Fold } from '$lib/components/fold/fold.svelte.ts';

  import TeamRow from './TeamRow.svelte';

  let {
    workspace,
    people,
    teams,
    membersChanged = 0,
    foldable = false,
  }: {
    workspace: Workspace;
    people: People;
    teams: Teams;
    membersChanged?: number;
    foldable?: boolean;
  } = $props();
  const id = $props.id();
  const workspaceId = $derived(workspace.info?.id);
  const managing = $derived(administers(workspace));
  const workspaceName = $derived(nameOfWorkspace(workspace.info, m.shell_workspace_unnamed));
  let groups = $state.raw<Team[]>([]);
  const fold = new Fold(() => !foldable || groups.length > 0 || unread);
  let members = $state.raw<WorkspaceMember[]>([]);
  const restrictedSelf = $derived(
    members.find((member) => member.userId === workspace.me?.id && member.joinedAsOrgAdmin)?.userId,
  );
  let loaded = $state(false);
  let working = $state(false);
  let problem = $state('');
  /** Whose name the last refusal was about: `CREATING` for the new team's, else a team's id. */
  let refusedName = $state<string | undefined>(undefined);
  const CREATING = 'creating';
  const problemId = `${id}-problem`;
  let unread = $state(false);
  let name = $state('');
  let deleting = $state.raw<Team | undefined>(undefined);
  let askOpen = $state(false);
  let reading = 0;

  async function read(place: string, manager: boolean): Promise<void> {
    const current = ++reading;
    // A failure to read the last workspace or board says nothing of this one.
    unread = false;
    try {
      const [listed, candidates] = await Promise.all([
        teams.list(place),
        manager ? people.members(place) : Promise.resolve([]),
      ]);
      if (current !== reading) return;
      groups = listed;
      members = candidates.filter((member) => member.role !== 'guest');
      loaded = true;
      unread = false;
    } catch (error) {
      if (isBackendError(error) && error.status === 401) workspace.report('load', error);
      else if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
        workspace.evictWorkspace(place);
      }
      if (current === reading) unread = true;
    }
  }

  // External I/O follows the workspace and permissions, including an open dialog.
  $effect(() => {
    if (workspaceId && membersChanged >= 0) void read(workspaceId, managing);
    return () => {
      reading += 1;
    };
  });

  /** The one line that says why a step was refused, by the reason the server gives. */
  function told(error: unknown): string {
    if (!isBackendError(error)) return m.groups_failed();
    if (error.reason === 'team_self_joining') return m.groups_self_joining();
    if (error.reason === 'team_name_taken') return m.groups_name_taken();
    switch (error.refusalAt('/name')?.reason) {
      case 'required':
        return m.groups_name_required();
      case 'too_long':
        return m.groups_name_too_long();
      case 'invalid_characters':
        return m.groups_name_invalid_characters();
      default:
        return m.groups_failed();
    }
  }

  /** Whether the refusal is about the name typed, which then is marked as refused. */
  function aboutName(error: unknown): boolean {
    return (
      isBackendError(error) &&
      (error.reason === 'team_name_taken' || error.refusalAt('/name') !== undefined)
    );
  }

  /**
   * Whether the step was done; on failure, what was typed for it stays. `naming`
   * says whose name the step sends, if it sends one.
   */
  async function act(step: (place: string) => Promise<unknown>, naming?: string): Promise<boolean> {
    const place = workspaceId;
    if (!place || working) return false;
    working = true;
    problem = '';
    refusedName = undefined;
    try {
      await step(place);
      workspace.evictWorkspace(place);
      return true;
    } catch (error) {
      if (isBackendError(error) && error.status === 401) workspace.report('save', error);
      else if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
        workspace.evictWorkspace(place);
      }
      problem = told(error);
      if (aboutName(error)) refusedName = naming;
      return false;
    } finally {
      await read(place, managing);
      working = false;
    }
  }
</script>

<section class="grid gap-3" aria-labelledby="{id}-title" data-testid="teams-panel">
  {#if foldable}
    <FoldHeading
      id="{id}-title"
      {fold}
      controls="{id}-body"
      title={m.groups_title()}
      count={groups.length}
      testId="teams-toggle"
    />
  {:else}
    <h3 id="{id}-title" class="font-medium">{m.groups_title()}</h3>
  {/if}
  {#if fold.open}
    <div id="{id}-body" class="grid gap-3">
      <p class="text-sm text-muted-foreground">
        {m.groups_description({ workspace: workspaceName })}
      </p>
      {#if problem || unread}<p id={problemId} role="alert" class="text-sm text-destructive">
          {problem || m.groups_load_failed()}
        </p>{/if}
      {#if managing}
        <form
          class="flex flex-wrap gap-2"
          onsubmit={(event) => {
            event.preventDefault();
            if (name.trim())
              void act(async (place) => {
                await teams.create(place, name);
                name = '';
              }, CREATING);
          }}
        >
          <input
            class="h-9 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 text-base md:text-sm"
            aria-label={m.groups_name()}
            placeholder={m.groups_name()}
            aria-invalid={refusedName === CREATING || undefined}
            aria-describedby={refusedName === CREATING ? problemId : undefined}
            bind:value={name}
            oninput={() => {
              if (refusedName === CREATING) refusedName = undefined;
            }}
            disabled={working}
          />
          <Button type="submit" size="sm" disabled={working || !name.trim()}>
            {m.groups_create()}
          </Button>
        </form>
      {/if}
      {#if loaded && groups.length === 0}<p class="text-sm text-muted-foreground">
          {m.groups_empty()}
        </p>{/if}
      <ul class="grid gap-2">
        {#each groups as team (team.id)}
          <TeamRow
            {team}
            {members}
            {managing}
            {working}
            {restrictedSelf}
            nameProblem={refusedName === team.id ? problemId : undefined}
            onNameEdit={() => {
              if (refusedName === team.id) refusedName = undefined;
            }}
            onRename={(value: string) =>
              act((place) => teams.rename(place, team.id, value), team.id)}
            onAdd={(userId: string) => act((place) => teams.addMember(place, team.id, userId))}
            onRemove={(userId: string) =>
              act((place) => teams.removeMember(place, team.id, userId))}
            onDelete={() => {
              deleting = team;
              askOpen = true;
            }}
          />
        {/each}
      </ul>
    </div>
  {/if}
</section>

<ConfirmDialog
  bind:open={askOpen}
  title={m.groups_delete_title({ name: deleting?.name ?? '' })}
  description={m.groups_delete_description()}
  confirmLabel={m.groups_delete()}
  destructive
  onConfirm={() => {
    if (deleting) {
      const team = deleting;
      void act((place) => teams.delete(place, team.id));
    }
  }}
/>
