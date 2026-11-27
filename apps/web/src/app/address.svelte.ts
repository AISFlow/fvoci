/**
 * The address and the workspace shown, kept together: the root address goes
 * to the workspace the reader starts in (`Workspace.firstChoice`), another
 * workspace in the address (a link, back or forward) is entered, and the one
 * entered is the one to come back to on this browser. Called while the app's
 * component is made: the effects are that component's.
 */
import { isOpen, type Workspace } from '$lib/state/index.ts';

import { rememberWorkspace } from './last-workspace.ts';
import { workspaceIdOf, type Router } from './router.svelte.ts';

export function followAddress(routerOf: () => Router, shown: () => Workspace | undefined): void {
  // The root address: to the workspace the reader starts in, in its place.
  $effect(() => {
    const [router, workspace] = [routerOf(), shown()];
    const route = router.route;
    if (workspace?.status !== 'ready') return;
    if (route.kind === 'meTime' || route.kind === 'todayHub') {
      if (!workspace.asked) {
        const chosen = workspace.firstChoice();
        if (chosen) void workspace.enter(chosen);
      }
      return;
    }
    if (route.kind !== 'home') return;
    const chosen = workspace.firstChoice();
    if (chosen) router.go({ workspaceId: chosen }, { replace: true });
  });

  // Another workspace in the address (or returning via Back after /org changes): entered.
  let checkedWorkspaces: Workspace['workspaces'] | undefined;
  $effect(() => {
    const workspace = shown();
    const asked = workspaceIdOf(routerOf().route);
    if (workspace?.status !== 'ready' || !asked) return;
    const workspacesChanged = workspace.workspaces !== checkedWorkspaces;
    checkedWorkspaces = workspace.workspaces;
    const listed = workspace.workspaces.find((one) => one.id === asked);
    const stale =
      workspacesChanged &&
      (listed
        ? (isOpen(listed) && workspace.entered === 'missing') ||
          (!isOpen(listed) && workspace.entered === 'ready')
        : workspace.entered === 'ready');
    if (asked !== workspace.asked || stale) {
      void workspace.enter(asked, stale);
    }
  });

  // The workspace entered is the one to come back to on this browser.
  $effect(() => {
    const workspace = shown();
    if (workspace?.entered === 'ready' && workspace.me && workspace.info) {
      rememberWorkspace(workspace.me.id, workspace.info.id);
    }
  });
}
