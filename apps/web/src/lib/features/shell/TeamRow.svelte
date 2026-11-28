<!-- A team stays one row until expanded, then shows its people. -->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { userDisplayName } from '$lib/logic/users.ts';
  import type { WorkspaceMember } from '$lib/state/people-types.ts';
  import type { Team } from '$lib/state/team-types.ts';
  import { Button } from '$lib/ui/button/index.js';

  let {
    team,
    members,
    managing,
    working,
    restrictedSelf,
    nameProblem,
    onNameEdit,
    onRename,
    onAdd,
    onRemove,
    onDelete,
  }: {
    team: Team;
    members: WorkspaceMember[];
    managing: boolean;
    working: boolean;
    restrictedSelf?: string;
    /** The id of the line that says why the new name was refused, while it says so. */
    nameProblem?: string;
    /** The new name is being changed: what was refused is no longer what is typed. */
    onNameEdit?: () => void;
    onRename: (name: string) => Promise<boolean>;
    onAdd: (userId: string) => Promise<boolean>;
    onRemove: (userId: string) => Promise<boolean>;
    onDelete: () => void;
  } = $props();
  let renaming = $state(false);
  let name = $state('');
  let selected = $state('');
  const candidates = $derived(
    members.filter((person) => !team.members.some((member) => member.userId === person.userId)),
  );
</script>

<li class="rounded-md border border-border" data-team={team.name}>
  <details>
    <summary class="cursor-pointer p-2 font-medium break-words">
      <span translate="no">{team.name}</span>
      <span class="text-sm text-muted-foreground">({team.members.length})</span>
    </summary>
    <div class="grid gap-3 border-t p-2">
      {#if managing}
        {#if renaming}
          <form
            class="flex flex-wrap gap-2"
            onsubmit={(event) => {
              event.preventDefault();
              if (name.trim())
                void onRename(name).then((done) => {
                  if (done) renaming = false;
                });
            }}
          >
            <input
              class="h-9 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 text-base md:text-sm"
              aria-label={m.groups_new_name({ name: team.name })}
              aria-invalid={nameProblem !== undefined || undefined}
              aria-describedby={nameProblem}
              bind:value={name}
              oninput={() => onNameEdit?.()}
              disabled={working}
            />
            <Button size="sm" type="submit" disabled={working || !name.trim()}>
              {m.groups_save()}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={working}
              onclick={() => {
                renaming = false;
              }}
            >
              {m.shared_cancel()}
            </Button>
          </form>
        {:else}
          <div class="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={working}
              onclick={() => {
                name = team.name;
                renaming = true;
              }}
            >
              {m.groups_rename()}
            </Button>
            <Button size="sm" variant="ghost" disabled={working} onclick={onDelete}>
              {m.groups_delete()}
            </Button>
          </div>
        {/if}
      {/if}
      {#if team.members.length === 0}<p class="text-sm text-muted-foreground">
          {m.groups_no_members()}
        </p>{/if}
      <ul class="grid gap-1">
        {#each team.members as member (member.userId)}
          <li class="flex min-h-9 items-center gap-2" data-team-member={member.userId}>
            <span class="min-w-0 flex-1 break-words" translate="no">{userDisplayName(member)}</span>
            {#if managing}
              <Button
                size="sm"
                variant="ghost"
                disabled={working}
                aria-label={m.groups_remove_member({ name: userDisplayName(member) })}
                onclick={() => void onRemove(member.userId)}
              >
                {m.people_remove()}
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
      {#if managing && restrictedSelf}
        <p class="text-sm text-muted-foreground">{m.groups_self_joining()}</p>
      {/if}
      {#if managing && candidates.length > 0}
        <form
          class="flex flex-wrap gap-2"
          onsubmit={(event) => {
            event.preventDefault();
            if (selected && selected !== restrictedSelf)
              void onAdd(selected).then((done) => {
                if (done) selected = '';
              });
          }}
        >
          <select
            class="h-9 min-w-0 flex-1 rounded-md border border-input-border bg-background px-2 text-base md:text-sm"
            aria-label={m.groups_choose_member()}
            bind:value={selected}
            disabled={working}
          >
            <option value="">{m.groups_choose_member()}</option>
            {#each candidates as person (person.userId)}
              <option value={person.userId} disabled={person.userId === restrictedSelf}>
                {userDisplayName(person)}
              </option>
            {/each}
          </select>
          <Button
            size="sm"
            type="submit"
            disabled={working || !selected || selected === restrictedSelf}
          >
            {m.groups_add_member()}
          </Button>
        </form>
      {/if}
    </div>
  </details>
</li>
