/**
 * Reactive global store for the signed-in reader's personal time tracker.
 *
 * Keeps the single active timer synchronized across app boot, tab focus /
 * `visibilitychange`, mutations, and same-browser tabs (`BroadcastChannel`
 * with safe fallback). Ticks once per second only while a timer is running.
 */
import { isBackendError, isPasswordSignInRequired, isSignedOut, ssoOrgOf } from './problems.ts';
import type {
  CreateTimeEntryRequest,
  ListTimeEntriesQuery,
  StartTimeEntryRequest,
  StartedTimeEntry,
  TimeEntry,
  TimeEntryList,
  TimeEntryPatch,
  TimeTracking,
} from './time-types.ts';
import type { Workspace } from './workspace.svelte.ts';

/** Notification emitted when starting a timer auto-stopped a previous visible timer. */
export interface SwitchedTimerNotice {
  id: number;
  stopped: TimeEntry;
  started: TimeEntry;
}

/** Session wall triggered by a time-tracking write (`401`). */
export type TimeSessionWall = { kind: 'password' } | { kind: 'sso'; orgId: string };

interface SyncMessage {
  type: 'sync';
  active: TimeEntry | null;
}

const TICK_MS = 1000;

export class TimeTracker {
  readonly #api: TimeTracking;
  readonly #workspace: Workspace;

  /** The currently running timer, or `null` when idle. */
  active = $state.raw<TimeEntry | null>(null);
  /** Wall-clock millisecond timestamp updated every second while `active !== null`. */
  now = $state(Date.now());
  /** True while a start or stop mutation is in flight. */
  busy = $state(false);
  /** Incremented whenever time entries change so open list views refresh. */
  version = $state(0);
  /** Most recent auto-switch notice (for toast feedback). */
  lastSwitched = $state.raw<SwitchedTimerNotice | null>(null);
  /** Active session wall requiring password confirmation or SSO sign-in. */
  wall = $state.raw<TimeSessionWall | null>(null);

  #switchSeq = 0;
  #tickHandle: ReturnType<typeof setInterval> | undefined;
  #channel: BroadcastChannel | undefined;
  #started = false;
  #refreshSeq = 0;

  constructor(api: TimeTracking, workspace: Workspace) {
    this.#api = api;
    this.#workspace = workspace;
  }

  /** Elapsed milliseconds of the currently running timer, or `0` when idle. */
  get elapsedMs(): number {
    return this.active ? Math.max(0, this.now - this.active.startedAt) : 0;
  }

  /** True if the active timer is tracking the given card or unit `blockId`. */
  isRunningFor(blockId: string): boolean {
    return this.active !== null && this.active.endedAt === null && this.active.blockId === blockId;
  }

  /**
   * Starts active-timer polling on boot, tab visibility/focus sync, and
   * same-browser `BroadcastChannel` sync. Returns a cleanup function.
   */
  start(): () => void {
    if (this.#started) return () => undefined;
    this.#started = true;
    void this.refreshActive();

    const onFocus = (): void => {
      void this.refreshActive();
    };
    const onVisibility = (): void => {
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
        void this.refreshActive();
      }
    };
    if (typeof window !== 'undefined') window.addEventListener('focus', onFocus);
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibility);
    }

    this.#openChannel();

    return () => {
      this.#started = false;
      this.#stopTick();
      if (typeof window !== 'undefined') window.removeEventListener('focus', onFocus);
      if (typeof document !== 'undefined') {
        document.removeEventListener('visibilitychange', onVisibility);
      }
      this.#closeChannel();
    };
  }

  /** Fetches the authoritative active timer from the backend. */
  async refreshActive(): Promise<TimeEntry | null> {
    const seq = ++this.#refreshSeq;
    try {
      const entry = await this.#api.getActiveTimeEntry();
      if (seq !== this.#refreshSeq) return this.active;
      const prev = this.active;
      this.#setActive(entry);
      if (entry?.id !== prev?.id || entry?.updatedAt !== prev?.updatedAt) {
        this.version += 1;
      }
      return entry;
    } catch {
      // Never disrupt the user with background refresh errors (or closed sessions handled elsewhere).
      return this.active;
    }
  }

  /** Starts a live timer, atomically stopping any previously running timer. */
  async startTimer(request: StartTimeEntryRequest): Promise<StartedTimeEntry | undefined> {
    if (this.busy) return undefined;
    this.busy = true;
    this.#refreshSeq += 1;
    try {
      const result = await this.#api.startTimeEntry(request);
      this.#refreshSeq += 1;
      this.#setActive(result.entry);
      this.version += 1;
      if (result.stopped) {
        this.#switchSeq += 1;
        this.lastSwitched = {
          id: this.#switchSeq,
          stopped: result.stopped,
          started: result.entry,
        };
      }
      this.#broadcast();
      return result;
    } catch (error) {
      if (!this.#captureWall(error)) this.#workspace.report('save', error);
      return undefined;
    } finally {
      this.busy = false;
    }
  }

  /** Stops the currently running timer. */
  async stopTimer(): Promise<TimeEntry | undefined> {
    if (this.busy) return undefined;
    this.busy = true;
    this.#refreshSeq += 1;
    try {
      const stopped = await this.#api.stopTimeEntry();
      this.#refreshSeq += 1;
      this.#setActive(null);
      this.version += 1;
      this.#broadcast();
      return stopped;
    } catch (error) {
      if (isBackendError(error) && error.status === 404) {
        this.#refreshSeq += 1;
        this.#setActive(null);
        this.version += 1;
        this.#broadcast();
        return undefined;
      }
      if (!this.#captureWall(error)) this.#workspace.report('save', error);
      return undefined;
    } finally {
      this.busy = false;
    }
  }

  /** Lists personal time entries for a filter range and page. */
  async listEntries(query: ListTimeEntriesQuery = {}): Promise<TimeEntryList> {
    try {
      return await this.#api.listTimeEntries(query);
    } catch (error) {
      if (isSignedOut(error)) this.#workspace.report('load', error);
      throw error;
    }
  }

  /** Manually logs a completed time entry. Re-throws validation errors for forms. */
  async createEntry(request: CreateTimeEntryRequest): Promise<TimeEntry> {
    try {
      const created = await this.#api.createTimeEntry(request);
      this.version += 1;
      this.#broadcast();
      return created;
    } catch (error) {
      if (!this.#captureWall(error) && isSignedOut(error)) this.#workspace.report('save', error);
      throw error;
    }
  }

  /** Updates an existing time entry. Re-throws validation errors for forms. */
  async patchEntry(entryId: string, patch: TimeEntryPatch): Promise<TimeEntry> {
    try {
      const updated = await this.#api.patchTimeEntry(entryId, patch);
      if (this.active?.id === entryId) {
        this.#refreshSeq += 1;
        this.#setActive(updated.endedAt === null ? updated : null);
      }
      this.version += 1;
      this.#broadcast();
      return updated;
    } catch (error) {
      if (!this.#captureWall(error) && isSignedOut(error)) this.#workspace.report('save', error);
      throw error;
    }
  }

  /** Deletes an existing time entry. Returns true on success, false on failure. */
  async deleteEntry(entryId: string): Promise<boolean> {
    try {
      await this.#api.deleteTimeEntry(entryId);
      if (this.active?.id === entryId) {
        this.#refreshSeq += 1;
        this.#setActive(null);
      }
      this.version += 1;
      this.#broadcast();
      return true;
    } catch (error) {
      if (!this.#captureWall(error)) this.#workspace.report('save', error);
      return false;
    }
  }

  /** Called after unlocking a session wall (e.g. password confirmation) to reload visible entries. */
  async refreshAfterUnlock(): Promise<void> {
    this.wall = null;
    this.version += 1;
    await this.refreshActive();
    this.#broadcast();
  }

  dismissSwitched(): void {
    this.lastSwitched = null;
  }

  clearWall(): void {
    this.wall = null;
  }

  #captureWall(error: unknown): boolean {
    if (isPasswordSignInRequired(error)) {
      this.wall = { kind: 'password' };
      return true;
    }
    const orgId = ssoOrgOf(error);
    if (orgId !== undefined) {
      this.wall = { kind: 'sso', orgId };
      return true;
    }
    return false;
  }

  #setActive(entry: TimeEntry | null): void {
    this.active = entry;
    this.now = Date.now();
    if (entry !== null) this.#startTick();
    else this.#stopTick();
  }

  #startTick(): void {
    if (this.#tickHandle !== undefined) return;
    this.#tickHandle = setInterval(() => {
      this.now = Date.now();
    }, TICK_MS);
  }

  #stopTick(): void {
    if (this.#tickHandle === undefined) return;
    clearInterval(this.#tickHandle);
    this.#tickHandle = undefined;
  }

  #openChannel(): void {
    if (typeof BroadcastChannel === 'undefined') return;
    const userId = this.#workspace.me?.id ?? 'self';
    try {
      const channel = new BroadcastChannel(`fvoci:time:${userId}`);
      channel.onmessage = (event: MessageEvent<unknown>) => {
        const data = event.data as Partial<SyncMessage> | null;
        if (data?.type !== 'sync') return;
        this.#setActive(data.active ?? null);
        this.version += 1;
        void this.refreshActive();
      };
      this.#channel = channel;
    } catch {
      // Restricted browser environments may disallow BroadcastChannel; focus sync still works.
      this.#channel = undefined;
    }
  }

  #closeChannel(): void {
    try {
      this.#channel?.close();
    } catch {
      // Ignore close errors.
    }
    this.#channel = undefined;
  }

  #broadcast(): void {
    if (!this.#channel) return;
    try {
      const message: SyncMessage = {
        type: 'sync',
        active: this.active ? { ...this.active } : null,
      };
      this.#channel.postMessage(message);
    } catch {
      // Safe fallback when BroadcastChannel cannot deliver.
    }
  }
}

const trackers = new WeakMap<Workspace, TimeTracker>();

/** Returns the per-workspace `TimeTracker`, or `undefined` when the backend has no time tracking. */
export function timeTracker(workspace: Workspace): TimeTracker | undefined {
  const api = workspace.backend.time;
  if (!api) return undefined;
  const existing = trackers.get(workspace);
  if (existing) return existing;
  const created = new TimeTracker(api, workspace);
  trackers.set(workspace, created);
  return created;
}
