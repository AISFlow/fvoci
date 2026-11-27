/**
 * What the reader did with the sidebar: folded it down to the strip (on an
 * expanded page), or slid it out as a drawer (on the others).
 *
 * Folding is a preference and is remembered on the device: it is about this
 * screen, not about the account. The drawer is not remembered; a page starts
 * with it closed.
 */

/** Where the device remembers whether the fixed sidebar is folded. */
export const SIDEBAR_STORAGE_KEY = 'fvoci:sidebar';

function remembered(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_STORAGE_KEY) === 'collapsed';
  } catch {
    return false;
  }
}

export class SidebarState {
  /** The fixed sidebar is folded down to the strip. */
  collapsed = $state(remembered());
  /** The drawer is out. */
  drawerOpen = $state(false);

  setCollapsed(collapsed: boolean): void {
    this.collapsed = collapsed;
    try {
      localStorage.setItem(SIDEBAR_STORAGE_KEY, collapsed ? 'collapsed' : 'open');
    } catch {
      // Not remembered: it still holds for this visit.
    }
  }
}
