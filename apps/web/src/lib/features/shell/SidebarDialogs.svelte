<!--
  Dialogs owned by `Sidebar.svelte`: workspace people, container trash, category
  naming, deletion confirmation, and cross-workspace container copy.
-->
<script lang="ts">
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Container } from '$lib/logic/wire.ts';
  import type { Category, PeopleAccess, Trash, Workspace } from '$lib/state/index.ts';

  import CategoryNameDialog from './CategoryNameDialog.svelte';
  import CopyToWorkspaceDialog from './CopyToWorkspaceDialog.svelte';
  import PeopleDialog from './PeopleDialog.svelte';
  import { shownTitle } from './shown-title.ts';
  import type { SidebarAsked } from './sidebar-actions.ts';
  import TrashDialog from './TrashDialog.svelte';

  interface Props {
    workspace: Workspace;
    access: PeopleAccess | undefined;
    peopleOpen: boolean;
    trash: Trash | undefined;
    trashOpen: boolean;
    naming: { mode: 'create' } | { mode: 'rename'; category: Category };
    namingOpen: boolean;
    onSubmitName: (name: string) => void;
    asked: SidebarAsked | undefined;
    askOpen: boolean;
    onConfirmDelete: () => void;
    copyingContainer: Container | undefined;
    copyOpen: boolean;
    onCopiedToWorkspace: (copied: Container) => void;
  }

  let {
    workspace,
    access,
    peopleOpen = $bindable(),
    trash,
    trashOpen = $bindable(),
    naming,
    namingOpen = $bindable(),
    onSubmitName,
    asked,
    askOpen = $bindable(),
    onConfirmDelete,
    copyingContainer,
    copyOpen = $bindable(),
    onCopiedToWorkspace,
  }: Props = $props();

  const askedKind = $derived(
    asked?.kind === 'container' ? (asked.container.kind ?? 'board') : undefined,
  );
  const titleOf = shownTitle;
</script>

{#if access}
  <PeopleDialog
    bind:open={peopleOpen}
    {workspace}
    people={access.people}
    inviteLink={access.inviteLink}
  />
{/if}

<TrashDialog
  bind:open={trashOpen}
  {trash}
  {workspace}
  title={m.shell_trash()}
  description={m.shell_trash_boards_retention({ days: workspace.trashRetentionDays })}
  untitled={m['Sidebar.untitled-board']()}
  testId="board-trash"
/>

<CategoryNameDialog
  bind:open={namingOpen}
  mode={naming.mode}
  initial={naming.mode === 'rename' ? naming.category.name : ''}
  onSubmit={onSubmitName}
/>

<ConfirmDialog
  bind:open={askOpen}
  title={asked?.kind === 'category'
    ? m['SidebarCategories.CategoryMenu.DeleteModal.Title']()
    : askedKind === 'wiki'
      ? m.shell_delete_wiki_title()
      : askedKind === 'diary'
        ? m.shell_delete_diary_title()
        : askedKind === 'course'
          ? m.shell_delete_course_title()
          : m['DeleteBoardDialog.confirm-tite']()}
  description={asked?.kind === 'category'
    ? m['SidebarCategories.CategoryMenu.DeleteModal.Body']({ categoryName: asked.category.name })
    : asked?.kind === 'container'
      ? askedKind === 'wiki'
        ? m.shell_delete_wiki_info({ title: titleOf(asked.container) })
        : askedKind === 'diary'
          ? m.shell_delete_diary_info({ title: titleOf(asked.container) })
          : askedKind === 'course'
            ? m.shell_delete_course_info({ title: titleOf(asked.container) })
            : m['DeleteBoardDialog.confirm-info']({ boardTitle: titleOf(asked.container) })
      : undefined}
  confirmLabel={asked?.kind === 'category'
    ? m.shared_delete()
    : m['DeleteBoardDialog.confirm-delete']()}
  destructive
  onConfirm={onConfirmDelete}
/>

<CopyToWorkspaceDialog
  bind:open={copyOpen}
  {workspace}
  container={copyingContainer}
  onCopied={onCopiedToWorkspace}
/>
