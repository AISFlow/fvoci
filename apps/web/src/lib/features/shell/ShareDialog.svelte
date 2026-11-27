<!--
  Sharing a board or a wiki, for its admins: who of the workspace may find and
  join it (read an open wiki unjoined) and with which role, its members and
  their roles, adding people of the workspace, and the public link (read only,
  for anyone who has it).

  Every change is sent at once through its store, which shows it and
  takes it back when the server refuses it. The people offered to add are
  the workspace's oldest ten until something is typed, then the ten the server
  finds by name (D2); members are left out. The public link is read when the
  dialog opens. The reader's own role is not changed here: that is another
  admin's to do, so that nobody locks themselves out by a slip.

  To an administrator of the workspace, who reads its people, a guest of it
  among the members is marked so, and is offered no admin role, which the
  server refuses them (`guest_role_limit`).

  A reader who is no admin sees its members and teams alone, with their roles,
  and changes nothing (`readOnly`). In the reader's own space, where nobody else
  works and there are no guests and no teams, its admin has the public link
  alone (`linkOnly`). Opened at the members (`at`, from the number
  of members at the head of the board), the dialog begins at that section.
  Guests and groups are folded while the board or the wiki has none.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import UserPlusIcon from '@lucide/svelte/icons/user-plus';
  import XIcon from '@lucide/svelte/icons/x';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import PersonProfile from '$lib/components/person/PersonProfile.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { userDisplayName } from '$lib/logic/users.ts';
  import {
    administers,
    isGuest,
    isOwnSpace,
    peopleOf,
    type ContainerRole,
    type Shareable,
    type User,
    type Workspace,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Switch } from '$lib/ui/switch/index.js';

  import ContainerTeams from './ContainerTeams.svelte';
  import GuestInvite from './GuestInvite.svelte';
  import { ROLE_ORDER, roleOfMember } from './share-logic.ts';
  import { shareWords } from './share-words.ts';
  import SharePublicLink from './SharePublicLink.svelte';
  import { shownSpaceName } from './space-names.ts';

  interface Props {
    open: boolean;
    workspace: Workspace;
    shareable: Shareable;
    /** The whole address of the public link with a read token. */
    linkOf: (readToken: string) => string;
    /**
     * The members alone: for a board or a wiki the reader manages without reading
     * it, or reads without being its admin, whose openness and public link are not
     * theirs to change or read.
     */
    membersOnly?: boolean;
    /** The members are shown, not changed: for a reader who is no admin of the board. */
    readOnly?: boolean;
    /** The section the dialog begins at, if not the top. */
    at?: 'members';
    /** Optional aggregate progress rate label for a course member (courses only). */
    memberProgress?: ((userId: string) => string | undefined) | undefined;
  }

  let {
    open = $bindable(),
    workspace,
    shareable,
    linkOf,
    membersOnly = false,
    readOnly = false,
    at,
    memberProgress,
  }: Props = $props();

  const workspaceName = $derived(shownSpaceName(workspace, m.shell_workspace_unnamed));
  /** The own space: what is in it is shown to others by its public link and nothing else. */
  const linkOnly = $derived(isOwnSpace(workspace.info) && !membersOnly && !readOnly);
  const words = $derived(shareWords(shareable.container?.kind));

  const uid = $props.id();
  const compact = $derived(page.stage === 'compact');

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );

  const ROLE_LABELS: Record<ContainerRole, () => string> = {
    admin: () => m['BoardMember.roleAdmin'](),
    editor: () => m['BoardMember.roleEditor'](),
    commenter: () => m['BoardMember.roleCommenter'](),
    viewer: () => m['BoardMember.roleViewer'](),
  };
  /** A minimum role is one a member can have below admin. */
  const MINIMUM_ROLES = ROLE_ORDER.filter((role) => role !== 'admin');

  const nameOf = (userId: string): string =>
    workspace.userName(userId) ?? m['centerPanel.unknown-user']();

  /** Inviting a guest by a link, where the backend has that. */
  const access = $derived(peopleOf(workspace));
  /** The guests of the workspace, read as the dialog opens: who of the members is one. */
  let guestIds = $state.raw<readonly string[]>([]);

  /** Of the workspace's people, which only its administrators read. */
  async function readGuests(): Promise<void> {
    const workspaceId = workspace.info?.id;
    if (!access || !workspaceId || linkOnly || !administers(workspace)) return;
    try {
      const people = await access.people.members(workspaceId);
      guestIds = people.filter((one) => one.role === 'guest').map((one) => one.userId);
    } catch {
      guestIds = [];
    }
  }

  const isOpen = $derived(shareable.container?.visibility === 'open');
  const minimumRole = $derived<ContainerRole>(
    shareable.container?.minimumRole && shareable.container.minimumRole !== 'admin'
      ? shareable.container.minimumRole
      : 'editor',
  );
  /** The role a person is added with: what joining gives on an open board, an editor otherwise. */
  const addedRole = $derived<ContainerRole>(isOpen ? minimumRole : 'editor');

  // ------------------------------------------------------- adding people

  let query = $state('');
  let found = $state.raw<User[]>([]);
  /** Readings of `found` begun: the latest is the one that counts. */
  let searches = 0;

  let searchField = $state<HTMLInputElement | null>(null);

  const candidates = $derived(
    (query.trim() === '' ? workspace.workspaceUsers : found).filter(
      (user) => !shareable.members.some((member) => member.userId === user.id && !member.synthetic),
    ),
  );

  async function search(text: string): Promise<void> {
    searches += 1;
    const reading = searches;
    if (text.trim() === '') {
      found = [];
      return;
    }
    const answer = await workspace.searchUsers(text.trim());
    if (reading === searches) found = answer;
  }

  let openVersion = $state(0);
  let membersHeading = $state<HTMLElement | null>(null);

  /** Opened at the members: the focus goes to their heading, which is scrolled to. */
  function focusAt(event: Event): void {
    if (at !== 'members' || !membersHeading) return;
    event.preventDefault();
    membersHeading.focus();
    membersHeading.scrollIntoView({ block: 'start' });
  }

  /** Each time the dialog opens (its content is made): a fresh search, the link read anew. */
  function opened(): void {
    untrack(() => {
      query = '';
      found = [];
      if (!membersOnly) openVersion += 1;
      void readGuests();
    });
  }

  /** The row that held the focus goes away: the focus goes to the search. */
  async function addMember(user: User): Promise<void> {
    searchField?.focus();
    await shareable.addMember(user.id, addedRole);
  }

  async function removeMember(userId: string): Promise<void> {
    searchField?.focus();
    await shareable.removeMember(userId);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    {@attach opened}
    onOpenAutoFocus={focusAt}
    data-testid="share-dialog"
    showCloseButton={false}
    class={[
      'flex flex-col gap-0 p-0',
      compact
        ? 'top-0 left-0 h-dvh max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] w-[min(32rem,calc(100%-2rem))] max-w-none sm:max-w-none',
    ]}
  >
    <Dialog.Header class="shrink-0 flex-row items-center gap-2 border-b py-2 pr-2 pl-4">
      <Dialog.Title class="min-w-0 flex-1 text-base font-semibold">
        {readOnly ? m.share_members_title() : words.title()}
      </Dialog.Title>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()}>
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    <div
      class="grid min-h-0 min-w-0 flex-1 grid-cols-1 content-start gap-5 overflow-y-auto overscroll-contain p-4"
    >
      <!-- The workspace. -->
      {#if !membersOnly && !linkOnly}
        <section class="grid gap-2" aria-labelledby="{uid}-workspace">
          <h3 id="{uid}-workspace" class="font-medium">
            {m.share_workspace_title({
              workspace: shownSpaceName(workspace, m.shell_workspace_unnamed_title),
            })}
          </h3>
          <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
            <span class="min-w-0 flex-1 text-muted-foreground"
              >{words.workspaceOpen({ workspace: workspaceName })}</span
            >
            <Switch
              checked={isOpen}
              data-testid="share-open"
              onCheckedChange={(checked: boolean) => void shareable.setOpen(checked)}
            />
          </label>
          {#if isOpen}
            <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
              <span class="min-w-0 flex-1 text-muted-foreground">{m.share_minimum_role()}</span>
              <select
                class="h-8 rounded-md border border-input-border bg-background px-2 text-base md:text-sm roomy:h-touch"
                data-testid="share-minimum-role"
                value={minimumRole}
                onchange={(event) =>
                  void shareable.setMinimumRole(event.currentTarget.value as ContainerRole)}
              >
                {#each MINIMUM_ROLES as role (role)}
                  <option value={role}>{ROLE_LABELS[role]()}</option>
                {/each}
              </select>
            </label>
          {/if}
        </section>
      {/if}

      {#if access && !readOnly && !linkOnly && !isGuest(workspace) && !shareable.container?.isTemplate}
        <GuestInvite
          {workspace}
          {access}
          containerId={shareable.containerId}
          kind={shareable.container?.kind}
          foldable
        />
      {/if}

      <!-- The members. -->
      {#if !linkOnly}
        <section class="grid grid-cols-1 gap-2" aria-labelledby="{uid}-members">
          <h3
            bind:this={membersHeading}
            id="{uid}-members"
            tabindex="-1"
            class="scroll-mt-4 font-medium outline-none"
            data-testid="share-members-title"
          >
            {m.share_members_title()}
          </h3>
          <ul class="grid grid-cols-1 gap-1" data-testid="share-members">
            {#each shareable.orderedMembers.filter((member) => !member.synthetic) as member (member.userId)}
              {@const name = nameOf(member.userId)}
              {@const mine = member.userId === workspace.me?.id}
              {@const memberRole = roleOfMember(member)}
              {@const person = workspace.user(member.userId)}
              {@const progress = memberProgress?.(member.userId)}
              {@const guest = guestIds.includes(member.userId)}
              <li class="flex min-h-8 items-center gap-2 roomy:min-h-touch" data-member={name}>
                <Avatar {name} colorKey={member.userId} locale={workspace.locale} decorative />
                <span class="flex min-w-0 flex-1 items-baseline gap-1">
                  {#if person}
                    <PersonProfile
                      userId={person.id}
                      username={person.username}
                      displayName={person.displayName}
                      name={person.name}
                      locale={workspace.locale}
                      goneName={name}
                    />
                  {:else}<span class="truncate">{name}</span>{/if}
                  {#if mine}<span class="shrink-0 text-muted-foreground">
                      {m['ShareBoard.userPermissionsYouText']()}</span
                    >{/if}
                  {#if guest}<span
                      class="shrink-0 self-center rounded-full border px-1.5 text-xs text-muted-foreground"
                      data-testid="share-guest-badge">{m.share_guest_badge()}</span
                    >{/if}
                </span>
                {#if progress}
                  <span
                    class="shrink-0 text-xs text-muted-foreground tabular-nums"
                    data-testid="member-course-progress"
                  >
                    {progress}
                  </span>
                {/if}
                {#if readOnly}
                  <span class="shrink-0 text-muted-foreground" data-testid="share-member-role">
                    {ROLE_LABELS[memberRole]()}
                  </span>
                {:else}
                  <!-- The reader's own role is another admin's to change: no locking oneself out. -->
                  <select
                    class="h-8 rounded-md border border-input-border bg-background px-2 text-base disabled:opacity-60 md:text-sm roomy:h-touch"
                    aria-label={m.share_role_of({ name })}
                    title={mine ? m.share_you_note() : undefined}
                    disabled={mine}
                    value={memberRole}
                    onchange={(event) =>
                      void shareable.setMemberRole(
                        member.userId,
                        event.currentTarget.value as ContainerRole,
                      )}
                  >
                    {#each guest ? MINIMUM_ROLES : ROLE_ORDER as role (role)}
                      <option value={role}>{ROLE_LABELS[role]()}</option>
                    {/each}
                  </select>
                  <IconButton
                    label={m.share_remove({ name })}
                    class="text-muted-foreground"
                    disabled={mine}
                    onclick={() => void removeMember(member.userId)}
                  >
                    <XIcon />
                  </IconButton>
                {/if}
              </li>
            {/each}
          </ul>

          {#if !readOnly}
            <Input
              bind:ref={searchField}
              type="search"
              bind:value={query}
              oninput={() => void search(query)}
              autocomplete="off"
              aria-label={m['ShareBoard.searchPlaceholder']()}
              placeholder={m['ShareBoard.searchPlaceholder']()}
              data-testid="share-search"
              class="roomy:h-touch roomy:text-base"
            />
            {#if candidates.length > 0}
              <ul class="grid grid-cols-1 gap-1" data-testid="share-candidates">
                {#each candidates as user (user.id)}
                  {@const name = userDisplayName(user)}
                  <li class="flex min-h-8 items-center gap-2 roomy:min-h-touch">
                    <Avatar {name} colorKey={user.id} locale={workspace.locale} decorative />
                    <span class="min-w-0 flex-1 truncate">{name}</span>
                    <Button
                      variant="outline"
                      size="sm"
                      class="roomy:min-h-touch"
                      aria-label={m.share_add({ name })}
                      onclick={() => void addMember(user)}
                    >
                      <UserPlusIcon aria-hidden="true" />
                      {m.share_add_short()}
                    </Button>
                  </li>
                {/each}
              </ul>
            {/if}
            <!-- In the page from the start: a live region that appears with its words is often not read. -->
            <p class="text-muted-foreground empty:hidden" role="status">
              {candidates.length === 0 && query.trim() !== ''
                ? m.share_nobody_found({ workspace: workspaceName })
                : ''}
            </p>
          {/if}
        </section>
      {/if}

      {#if access?.people.teams && !linkOnly && !isGuest(workspace)}
        <ContainerTeams
          {workspace}
          {shareable}
          teams={access.people.teams}
          {readOnly}
          {memberProgress}
          foldable
        />
      {/if}

      <!-- The public link. -->
      {#if !membersOnly}
        <SharePublicLink
          {workspace}
          containerId={shareable.containerId}
          {linkOf}
          {linkOnly}
          {openVersion}
        />
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
