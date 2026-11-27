<!--
  Global host for the active personal timer:
  - Starts/stops `TimeTracker` lifecycle sync (boot, focus/visibility, BroadcastChannel).
  - Renders `ActiveTimerPill` in the top bar whenever a timer is running.
  - Shows a polite toast when starting a timer automatically stopped a previous one.
  - Reuses `OwnSpaceLocked` / `SsoRequired` dialogs when a mutation hits a 401 session wall.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import XIcon from '@lucide/svelte/icons/x';

  import OwnSpaceLocked from '$lib/features/account/OwnSpaceLocked.svelte';
  import SsoRequired from '$lib/features/account/SsoRequired.svelte';
  import ActiveTimerPill from '$lib/features/time/ActiveTimerPill.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    confirmPassword,
    timeTracker,
    type Workspace,
    type WorkspaceInfo,
  } from '$lib/state/index.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import type { Router } from './router.svelte.ts';
  import { openTimeEntryTarget, timeEntryTargetHref } from './time-places.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
    openWorkspaces: readonly (WorkspaceInfo & { href: string })[];
    onOpenLinked: () => void;
    onSso: (orgId: string) => Promise<void>;
  }

  let { workspace, router, openWorkspaces, onOpenLinked, onSso }: Props = $props();

  const SWITCH_TOAST_MS = 6000;

  const tracker = $derived(workspace.me?.id ? timeTracker(workspace) : undefined);

  $effect(() => {
    const current = tracker;
    return current ? untrack(() => current.start()) : undefined;
  });

  $effect(() => {
    const current = tracker;
    const notice = current?.lastSwitched;
    if (!current || !notice) return;
    const handle = setTimeout(() => {
      if (current.lastSwitched?.id === notice.id) current.dismissSwitched();
    }, SWITCH_TOAST_MS);
    return () => {
      clearTimeout(handle);
    };
  });

  const pillHref = $derived(
    tracker?.active
      ? (timeEntryTargetHref(router, tracker.active) ?? router.href({ meTime: true }))
      : router.href({ meTime: true }),
  );

  const wallOrgName = $derived.by(() => {
    const wall = tracker?.wall;
    if (wall?.kind !== 'sso') return '';
    return workspace.workspaces.find((one) => one.orgId === wall.orgId)?.title ?? '';
  });

  async function confirmForTime(password: string): Promise<void> {
    await confirmPassword(workspace, password);
    await tracker?.refreshAfterUnlock();
  }
</script>

{#if tracker?.active}
  {@const active = tracker.active}
  <ActiveTimerPill
    entry={active}
    elapsedMs={tracker.elapsedMs}
    busy={tracker.busy}
    href={pillHref}
    onOpen={() => {
      openTimeEntryTarget(router, active);
    }}
    onStop={() => void tracker.stopTimer()}
  />
{/if}

{#if tracker?.lastSwitched}
  <div
    class="pointer-events-none fixed inset-x-0 bottom-[calc(var(--safe-bottom)+var(--keyboard)+3.5rem)] z-[60] flex justify-center px-3"
    role="status"
    aria-live="polite"
    data-testid="time-switch-toast"
  >
    <div
      class="pointer-events-auto flex max-w-md items-center gap-2 rounded-lg border bg-popover py-2 pr-2 pl-3 text-sm text-popover-foreground shadow-md"
    >
      <p class="min-w-0 flex-1">{m.time_switched_toast()}</p>
      <button
        type="button"
        class="grid size-8 shrink-0 place-items-center rounded-md hover:bg-muted roomy:size-touch"
        aria-label={m.app_problem_dismiss()}
        onclick={() => {
          tracker.dismissSwitched();
        }}
      >
        <XIcon class="size-4" aria-hidden="true" />
      </button>
    </div>
  </div>
{/if}

{#if tracker?.wall}
  {@const wall = tracker.wall}
  <Dialog.Root
    bind:open={
      () => tracker.wall !== null,
      (open: boolean) => {
        if (!open) tracker.clearWall();
      }
    }
  >
    <Dialog.Content class="max-w-md" data-testid="time-session-wall">
      <Dialog.Title class="sr-only">
        {wall.kind === 'password' ? m.time_wall_password_title() : m.time_wall_sso_title()}
      </Dialog.Title>
      {#if wall.kind === 'password'}
        <OwnSpaceLocked username={workspace.me?.username ?? ''} onConfirm={confirmForTime} />
      {:else}
        <SsoRequired
          name={wallOrgName}
          others={openWorkspaces}
          onContinue={() => onSso(wall.orgId)}
          onOpenLinked={() => {
            tracker.clearWall();
            onOpenLinked();
          }}
          onGo={(workspaceId: string) => {
            tracker.clearWall();
            router.go({ workspaceId });
          }}
        />
      {/if}
    </Dialog.Content>
  </Dialog.Root>
{/if}
