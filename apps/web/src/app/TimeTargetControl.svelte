<!--
  Connects a board card or course unit to the global `TimeTracker`.
  Renders nothing when unauthenticated or on public shared links (`backend.time === undefined`).
-->
<script lang="ts">
  import TargetTimerButton from '$lib/features/time/TargetTimerButton.svelte';
  import type { TimeEntryKind } from '$lib/logic/time.ts';
  import { timeTracker, type Workspace } from '$lib/state/index.ts';

  interface Props {
    workspace: Workspace;
    workspaceId: string | undefined;
    containerId: string;
    blockId: string;
    defaultKind: TimeEntryKind;
  }

  let { workspace, workspaceId, containerId, blockId, defaultKind }: Props = $props();

  const tracker = $derived(workspace.me?.id ? timeTracker(workspace) : undefined);
  const running = $derived(tracker?.isRunningFor(blockId) ?? false);

  function start(kind: TimeEntryKind): void {
    if (!tracker) return;
    void tracker.startTimer({
      ...(workspaceId ? { workspaceId } : {}),
      containerId,
      blockId,
      kind,
    });
  }
</script>

{#if tracker}
  <TargetTimerButton
    {running}
    elapsedMs={running ? tracker.elapsedMs : 0}
    busy={tracker.busy}
    {defaultKind}
    onStart={start}
    onStop={() => void tracker.stopTimer()}
  />
{/if}
