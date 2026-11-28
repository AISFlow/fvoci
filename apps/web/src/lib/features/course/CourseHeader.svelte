<!--
  The head of a course's left column: its icon, name and description, a
  reserved slot for L2 "My progress", sharing/members, the trash button for
  editors, and the course menu: a new meeting or exam for editors (whose tabs
  show once there is one), edit / delete for managers.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import ClipboardCheckIcon from '@lucide/svelte/icons/clipboard-check';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import Share2Icon from '@lucide/svelte/icons/share-2';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import UsersIcon from '@lucide/svelte/icons/users';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CourseStore } from '$lib/state/index.ts';

  interface Props {
    course: CourseStore;
    onEdit: () => void;
    onDelete: () => void;
    onTrash: () => void;
    onShare?: (() => void) | undefined;
    /** Opens the making of a meeting, where the course has its tabs. */
    onNewMeeting?: (() => void) | undefined;
    /** Opens the making of an exam, where the course has its tabs. */
    onNewExam?: (() => void) | undefined;
    /** Reserved header slot for L2 "My progress". */
    progress?: Snippet | undefined;
  }

  let { course, onEdit, onDelete, onTrash, onShare, onNewMeeting, onNewExam, progress }: Props =
    $props();

  const title = $derived(course.course?.title || m.course_untitled());
  const making = $derived<MenuAction[]>(
    course.canEdit
      ? [
          ...(onNewMeeting
            ? [
                {
                  id: 'add-meeting',
                  label: m.course_menu_new_meeting(),
                  icon: UsersIcon,
                  afterClose: onNewMeeting,
                },
              ]
            : []),
          ...(onNewExam
            ? [
                {
                  id: 'add-exam',
                  label: m.course_exams_new(),
                  icon: ClipboardCheckIcon,
                  afterClose: onNewExam,
                },
              ]
            : []),
        ]
      : [],
  );
  const managing = $derived<MenuAction[]>(
    course.canManage
      ? [
          {
            id: 'edit-course',
            label: m.course_settings_title(),
            icon: PencilIcon,
            run: onEdit,
            separated: making.length > 0,
          },
          {
            id: 'delete-course',
            label: m.shell_delete_course(),
            icon: Trash2Icon,
            destructive: true,
            separated: true,
            run: onDelete,
          },
        ]
      : [],
  );
  const actions = $derived([...making, ...managing]);
</script>

<div class="flex shrink-0 flex-col gap-2 border-b px-3 py-3" data-testid="course-header">
  <div class="flex items-start gap-2">
    {#if course.course?.icon}
      <span class="text-xl leading-7" aria-hidden="true" data-testid="course-header-icon">
        {course.course.icon}
      </span>
    {/if}
    <div class="min-w-0 flex-1">
      <p class="truncate leading-7 font-semibold" data-testid="course-header-title">{title}</p>
      {#if course.course?.description}
        <p
          class="line-clamp-3 text-sm whitespace-pre-line text-muted-foreground"
          data-testid="course-header-description"
        >
          {course.course.description}
        </p>
      {/if}
    </div>
    {#if onShare}
      <IconButton
        label={course.canManage ? m.shell_share() : m.share_members_title()}
        data-testid="share-course"
        aria-haspopup="dialog"
        class="text-muted-foreground"
        onclick={onShare}
      >
        <Share2Icon />
      </IconButton>
    {/if}
    {#if course.canEdit}
      <IconButton
        label={m.course_trash()}
        data-testid="open-unit-trash"
        aria-haspopup="dialog"
        class="text-muted-foreground"
        onclick={onTrash}
      >
        <Trash2Icon />
      </IconButton>
    {/if}
    {#if actions.length > 0}
      <ActionMenu {title} align="end" {actions}>
        {#snippet trigger(props: Record<string, unknown>)}
          <IconButton
            {...props}
            label={m.course_menu({ title })}
            data-testid="course-menu"
            class="text-muted-foreground"
          >
            <EllipsisIcon />
          </IconButton>
        {/snippet}
      </ActionMenu>
    {/if}
  </div>
  <!-- Reserved slot for L2 "My progress". -->
  <div data-slot="course-progress" class="empty:hidden">
    {@render progress?.()}
  </div>
</div>
