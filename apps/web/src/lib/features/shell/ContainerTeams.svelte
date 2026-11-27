<!--
  The teams of a board or a wiki: one row per team, shaped as a member's row;
  direct memberships remain in the people list. Shown, not changed, to a
  reader who is no admin of it (`readOnly`). Among the other parts of the
  sharing (`foldable`), it is folded until a group is on it.
-->
<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import UserPlusIcon from '@lucide/svelte/icons/user-plus';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError, type Shareable, type Workspace } from '$lib/state/index.ts';
  import type { ContainerTeam, Team, TeamRole, Teams } from '$lib/state/team-types.ts';
  import { Button } from '$lib/ui/button/index.js';
  import FoldHeading from '$lib/components/fold/FoldHeading.svelte';
  import { Fold } from '$lib/components/fold/fold.svelte.ts';

  import { shareWords } from './share-words.ts';

  interface Props {
    workspace: Workspace;
    shareable: Shareable;
    teams: Teams;
    readOnly?: boolean;
    memberProgress?: ((userId: string) => string | undefined) | undefined;
    foldable?: boolean;
  }

  let {
    workspace,
    shareable,
    teams,
    readOnly = false,
    memberProgress,
    foldable = false,
  }: Props = $props();
  const words = $derived(shareWords(shareable.container?.kind));
  const id = $props.id();
  const place = $derived(workspace.info?.id);
  let assigned = $state.raw<ContainerTeam[]>([]);
  let groups = $state.raw<Team[]>([]);
  let selected = $state('');
  let role = $state<TeamRole>('editor');
  let busy = $state(false);
  let problem = $state('');
  let unread = $state(false);
  const fold = new Fold(() => !foldable || assigned.length > 0 || unread);
  let reading = 0;
  const candidates = $derived(
    groups.filter((team) => !assigned.some((item) => item.team.id === team.id)),
  );
  const roles: TeamRole[] = ['editor', 'commenter', 'viewer'];
  const labels = {
    editor: () => m['BoardMember.roleEditor'](),
    commenter: () => m['BoardMember.roleCommenter'](),
    viewer: () => m['BoardMember.roleViewer'](),
  };

  async function read(workspaceId: string, containerId: string): Promise<void> {
    const current = ++reading;
    // A failure to read the last workspace or board says nothing of this one.
    unread = false;
    try {
      const [all, onContainer] = await Promise.all([
        teams.list(workspaceId),
        teams.onContainer(workspaceId, containerId),
      ]);
      if (current !== reading) return;
      groups = all;
      assigned = onContainer;
      unread = false;
    } catch (error) {
      if (isBackendError(error)) {
        if (error.status === 401) workspace.report('load', error);
        else if (error.status === 403 || error.status === 404) {
          workspace.evictWorkspace(workspaceId);
        }
      }
      if (current === reading) unread = true;
    }
  }

  $effect(() => {
    // Member messages also refresh this external view after another person's changes.
    const request = { place, containerId: shareable.containerId, members: shareable.members };
    if (request.place) void read(request.place, request.containerId);
    return () => {
      reading += 1;
    };
  });

  /** Whether the change was made. */
  async function change(teamId: string, next: TeamRole | undefined): Promise<boolean> {
    if (!place || busy) return false;
    busy = true;
    problem = '';
    try {
      if (next) await teams.assign(place, shareable.containerId, teamId, next);
      else await teams.unassign(place, shareable.containerId, teamId);
      selected = '';
      return true;
    } catch (error) {
      if (isBackendError(error)) {
        if (error.status === 401) workspace.report('save', error);
        else if (error.status === 403 || error.status === 404) workspace.evictWorkspace(place);
      }
      problem = m.groups_failed();
      return false;
    } finally {
      await read(place, shareable.containerId);
      busy = false;
    }
  }
</script>

<!-- A reader who changes nothing is not shown an empty list of teams. -->
{#if !readOnly || assigned.length > 0 || unread}
  <section class="grid grid-cols-1 gap-2" aria-labelledby="{id}-title" data-testid="board-teams">
    {#if foldable}
      <FoldHeading
        id="{id}-title"
        {fold}
        controls="{id}-body"
        title={m.groups_title()}
        count={assigned.length}
        testId="board-teams-toggle"
      />
    {:else}
      <h3 id="{id}-title" class="font-medium">{m.groups_title()}</h3>
    {/if}
    {#if fold.open}
      <div id="{id}-body" class="grid grid-cols-1 gap-2">
        <p class="text-sm text-muted-foreground">{m.groups_board_description()}</p>
        {#if problem || unread}<p role="alert" class="text-sm text-destructive">
            {problem || m.groups_load_failed()}
          </p>{/if}
        <ul class="grid grid-cols-1 gap-1">
          {#each assigned as item (item.team.id)}
            <li class="flex items-start gap-2" data-board-team={item.team.name}>
              <details class="group min-w-0 flex-1">
                <summary
                  class="flex min-h-8 cursor-pointer list-none items-center gap-2 roomy:min-h-touch [&::-webkit-details-marker]:hidden"
                >
                  <!-- The chevron is the marker: WebKit would draw its own beside it. -->
                  <ChevronRightIcon
                    class="size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90"
                    aria-hidden="true"
                  />
                  <span class="min-w-0 truncate" translate="no">{item.team.name}</span>
                  <span class="text-sm text-muted-foreground">({item.team.members.length})</span>
                </summary>
                <ul class="grid grid-cols-1 gap-1 pb-1 pl-6 text-sm">
                  {#each item.team.members as person (person.userId)}
                    {@const progress = memberProgress?.(person.userId)}
                    <li class="flex items-baseline justify-between gap-2">
                      <span class="min-w-0 break-words" translate="no">
                        {person.displayName || person.username}
                      </span>
                      {#if progress}
                        <span
                          class="shrink-0 text-xs text-muted-foreground tabular-nums"
                          data-testid="team-member-course-progress"
                        >
                          {progress}
                        </span>
                      {/if}
                    </li>
                  {/each}
                </ul>
              </details>
              {#if readOnly}
                <span
                  class="flex min-h-8 shrink-0 items-center text-muted-foreground roomy:min-h-touch"
                >
                  {labels[item.role]()}
                </span>
              {:else}
                <div class="flex min-h-8 items-center gap-2 roomy:min-h-touch">
                  <select
                    class="h-8 rounded-md border border-input-border bg-background px-2 text-base disabled:opacity-60 md:text-sm roomy:h-touch"
                    aria-label={m.share_role_of({ name: item.team.name })}
                    value={item.role}
                    disabled={busy}
                    onchange={(event) => {
                      const select = event.currentTarget;
                      void change(item.team.id, select.value as TeamRole).then((done) => {
                        // A refused role is not left showing as if it were the team's.
                        if (!done) select.value = item.role;
                      });
                    }}
                  >
                    {#each roles as option (option)}<option value={option}
                        >{labels[option]()}</option
                      >{/each}
                  </select>
                  <IconButton
                    label={words.teamRemove({ name: item.team.name })}
                    class="text-muted-foreground"
                    disabled={busy}
                    onclick={() => void change(item.team.id, undefined)}
                  >
                    <XIcon />
                  </IconButton>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
        {#if candidates.length > 0 && !readOnly}
          <form
            class="flex flex-wrap gap-2"
            onsubmit={(event) => {
              event.preventDefault();
              if (selected) void change(selected, role);
            }}
          >
            <select
              class="h-8 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 text-base disabled:opacity-60 md:text-sm roomy:h-touch"
              aria-label={m.groups_board_choose()}
              bind:value={selected}
              disabled={busy}
            >
              <option value="">{m.groups_board_choose()}</option>
              {#each candidates as team (team.id)}<option value={team.id}>{team.name}</option
                >{/each}
            </select>
            <select
              class="h-8 rounded-md border border-input-border bg-background px-2 text-base disabled:opacity-60 md:text-sm roomy:h-touch"
              aria-label={m.share_minimum_role()}
              bind:value={role}
              disabled={busy}
            >
              {#each roles as option (option)}<option value={option}>{labels[option]()}</option
                >{/each}
            </select>
            <Button
              type="submit"
              variant="outline"
              size="sm"
              class="roomy:min-h-touch"
              disabled={busy || !selected}
            >
              <UserPlusIcon aria-hidden="true" />
              {m.groups_board_add()}
            </Button>
          </form>
        {/if}
      </div>
    {/if}
  </section>
{/if}
