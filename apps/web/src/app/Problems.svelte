<!--
  Tells the reader what went wrong: a change that was refused and taken back,
  a reading or an archive that failed. One line each, in a live region, until
  dismissed or a while has passed. A file that did not upload is told where
  its upload is shown (features/shell/Transfers.svelte), not here.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import { reloadForNewVersion, staleBuild } from '$lib/chunk-reload.svelte.ts';
  import { nameOfWorkspace } from '$lib/components/workspace-name.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { humanFileSize } from '$lib/logic/format.ts';
  import type { Problem, Workspace } from '$lib/state/index.ts';

  let { workspace }: { workspace: Workspace } = $props();

  const workspaceName = $derived(nameOfWorkspace(workspace.info, m.shell_workspace_unnamed));

  /** How long a line stays when nobody dismisses it, in milliseconds. */
  const STAY = 8000;

  function text(problem: Problem): string {
    const error = problem.error;
    // No answer at all.
    if (error?.status === 0) return m.app_problem_network();
    if (problem.action === 'import' && error?.status === 403) {
      return m.app_problem_import_forbidden({ workspace: workspaceName });
    }
    if (error?.status === 403) return m.app_problem_forbidden();
    // The limits of a wiki's tree, which nothing in the request could have helped.
    if (error?.reason === 'page_too_deep' && error.limit) {
      return m.app_problem_page_too_deep({ limit: error.limit });
    }
    if (error?.reason === 'card_too_deep') {
      return m.app_problem_card_too_deep({ limit: error.limit ?? 3 });
    }
    if (error?.reason === 'too_many_pages' && error.limit) {
      return m.app_problem_too_many_pages({ limit: error.limit });
    }
    if (error?.reason === 'too_many_units' && error.limit) {
      return m.app_problem_too_many_units({ limit: error.limit });
    }
    // A diary's day holds one entry: one from the trash finds it taken.
    if (error?.reason === 'day_taken') return m.app_problem_day_taken();
    if (error?.reason === 'too_many_chapters' && error.limit) {
      return m.app_problem_too_many_chapters({ limit: error.limit });
    }
    if (problem.action === 'import' && error?.status === 413) {
      return error.reason === 'archive_too_large' && error.limit
        ? m.app_problem_import_too_large({ limit: humanFileSize(error.limit) })
        : m.app_problem_import_too_big();
    }
    if (problem.action === 'import' && error?.status === 400) {
      return m.app_problem_import_invalid();
    }
    if (problem.action === 'import' && error?.status === 429) {
      return m.app_problem_upload_busy();
    }
    switch (problem.action) {
      // Answered 404: a page somebody else deleted meanwhile, taken off the tree.
      case 'movePage':
        return m.app_problem_page_move_gone();
      case 'moveUnit':
        return m.app_problem_unit_move_gone();
      case 'export':
        return m.app_problem_export();
      case 'import':
        return m.app_problem_import();
      case 'load':
        return m.app_problem_load();
      case 'undo':
        return m.app_problem_undo();
      default:
        return m.app_problem_save();
    }
  }

  $effect(() => {
    const timers = workspace.problems.map((problem) =>
      setTimeout(() => {
        workspace.dismissProblem(problem.id);
      }, STAY),
    );
    return () => {
      for (const timer of timers) clearTimeout(timer);
    };
  });
</script>

<div
  class="pointer-events-none fixed inset-x-0 bottom-[calc(var(--safe-bottom)+var(--keyboard)+0.75rem)] z-[60] flex flex-col items-center gap-2 pr-[calc(var(--safe-right)+0.75rem)] pl-[calc(var(--safe-left)+0.75rem)]"
  role="status"
  aria-live="polite"
  data-testid="problems"
>
  {#if staleBuild.detected}
    <button
      type="button"
      class="pointer-events-auto max-w-md rounded-lg border bg-popover px-3 py-2 text-left text-sm font-medium text-popover-foreground shadow-md hover:bg-muted roomy:min-h-touch"
      data-testid="new-version-banner"
      onclick={() => void reloadForNewVersion()}
    >
      {m['BoardPage.newVersion']()}
    </button>
  {/if}
  {#each workspace.problems as problem (problem.id)}
    <div
      class="pointer-events-auto flex max-w-md items-center gap-2 rounded-lg border bg-popover py-2 pr-2 pl-3 text-popover-foreground shadow-md"
      data-problem={problem.action}
    >
      <p class="min-w-0 flex-1">{text(problem)}</p>
      <button
        type="button"
        class="grid size-8 shrink-0 place-items-center rounded-md hover:bg-muted roomy:size-touch"
        aria-label={m.app_problem_dismiss()}
        onclick={() => {
          workspace.dismissProblem(problem.id);
        }}
      >
        <XIcon class="size-4" aria-hidden="true" />
      </button>
    </div>
  {/each}
</div>
