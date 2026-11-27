/**
 * Sidebar writes under way (`Workspace`). A reading of the sidebar meanwhile
 * may be older than what they show (a filing is two requests, and the first
 * one's message has the sidebar read between them): it is not shown, and the
 * sidebar is read again once the last write has ended.
 */
import type { Category } from './backend.ts';

export class SidebarWrites {
  /** Writes ever begun: a reading begun before one of them is older than it. */
  begun = 0;
  #underWay = 0;
  #stale = false;
  readonly #readAgain: () => void;

  /** `readAgain` reads the sidebar anew. */
  constructor(readAgain: () => void) {
    this.#readAgain = readAgain;
  }

  /** Earlier echoes wait while a write is pending, just as a reading does. */
  accepts(categories: readonly Category[], workspaceId: string | undefined): boolean {
    return (
      categories.every((one) => one.workspaceId === workspaceId) &&
      this.reading(this.begun) === 'show'
    );
  }

  begin(): void {
    this.#underWay += 1;
    this.begun += 1;
  }

  end(): void {
    this.#underWay -= 1;
    if (this.#underWay > 0 || !this.#stale) return;
    this.#stale = false;
    this.#readAgain();
  }

  /**
   * What to do with a reading begun when `begun` writes had: show it; leave
   * it, to be read again once the writes under way end (`later`); or read
   * again now, a write having begun since (`again`).
   */
  reading(begun: number): 'show' | 'later' | 'again' {
    if (this.#underWay > 0) {
      this.#stale = true;
      return 'later';
    }
    return begun === this.begun ? 'show' : 'again';
  }
}
