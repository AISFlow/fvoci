<!--
  App-supplied top-bar tools rendered inside `AppShell`'s `search` slot:
  active personal timer pill, workspace search, and notification bell.
-->
<script lang="ts">
  import type { Workspace, WorkspaceInfo } from '$lib/state/index.ts';

  import NotificationsHost from './NotificationsHost.svelte';
  import type { Router } from './router.svelte.ts';
  import TimeHost from './TimeHost.svelte';
  import WorkspaceSearch from './WorkspaceSearch.svelte';

  interface Props {
    workspace: Workspace;
    router: Router;
    openWorkspaces: readonly (WorkspaceInfo & { href: string })[];
    onOpenLinked: () => void;
    onSso: (orgId: string) => Promise<void>;
  }

  let { workspace, router, openWorkspaces, onOpenLinked, onSso }: Props = $props();
</script>

<TimeHost {workspace} {router} {openWorkspaces} {onOpenLinked} {onSso} />
<WorkspaceSearch {workspace} {router} />
<NotificationsHost {workspace} {router} />
