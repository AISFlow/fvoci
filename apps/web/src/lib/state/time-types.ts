/**
 * Personal time tracking types and backend boundary (`/api/v1/users/me/time/*`).
 *
 * Strictly owner-only: no organization rollups, no cross-user visibility, and absent
 * (`undefined`) on public shared links (`SharedBackend`).
 */
import type { TimeEntryKind } from '../logic/time.ts';

export type { TimeEntryKind } from '../logic/time.ts';

/** Resource kind captured when a time entry is linked to a target. */
export type TimeTargetKind = 'card' | 'unit' | 'board' | 'course';

/** One personal time tracking entry. */
export interface TimeEntry {
  id: string;
  workspaceId: string | null;
  containerId: string | null;
  blockId: string | null;
  targetKind: TimeTargetKind | null;
  /** Snapshot of the target's title at creation time; survives target deletion. */
  targetTitle: string | null;
  kind: TimeEntryKind;
  startedAt: number;
  /** `null` while the timer is actively running. */
  endedAt: number | null;
  note: string;
  createdAt: number;
  updatedAt: number;
}

/** Result of starting a live timer, including any visible timer auto-stopped in the transaction. */
export interface StartedTimeEntry {
  entry: TimeEntry;
  stopped?: TimeEntry;
}

/** Completed milliseconds aggregated by activity kind. */
export interface TimeEntryKindTotals {
  lecture: number;
  study: number;
  work: number;
  other: number;
}

/** Summary across all matching entries across all pages. */
export interface TimeEntrySummary {
  totalMs: number;
  byKind: TimeEntryKindTotals;
  count: number;
}

/** One page of time entries paired with the full-filter summary. */
export interface TimeEntryList {
  entries: TimeEntry[];
  summary: TimeEntrySummary;
}

export interface StartTimeEntryRequest {
  workspaceId?: string | null;
  containerId?: string | null;
  blockId?: string | null;
  kind: TimeEntryKind;
  note?: string;
}

export interface CreateTimeEntryRequest {
  workspaceId?: string | null;
  containerId?: string | null;
  blockId?: string | null;
  kind: TimeEntryKind;
  startedAt: number;
  endedAt: number;
  note?: string;
}

/** Partial patch for an entry; unchanged fields are omitted (explicit `null` is forbidden). */
export interface TimeEntryPatch {
  kind?: TimeEntryKind;
  startedAt?: number;
  endedAt?: number;
  note?: string;
}

export interface ListTimeEntriesQuery {
  from?: number;
  to?: number;
  kind?: TimeEntryKind;
  blockId?: string;
  containerId?: string;
  page?: number;
  perPage?: number;
}

/** Personal time tracking operations on a backend. */
export interface TimeTracking {
  getActiveTimeEntry(): Promise<TimeEntry | null>;
  startTimeEntry(request: StartTimeEntryRequest): Promise<StartedTimeEntry>;
  stopTimeEntry(): Promise<TimeEntry>;
  listTimeEntries(query?: ListTimeEntriesQuery): Promise<TimeEntryList>;
  createTimeEntry(request: CreateTimeEntryRequest): Promise<TimeEntry>;
  patchTimeEntry(entryId: string, patch: TimeEntryPatch): Promise<TimeEntry>;
  deleteTimeEntry(entryId: string): Promise<void>;
}

/** Where a backend keeps the signed-in reader's personal time tracking; absent on shared links. */
export interface TimeOfBackend {
  readonly time: TimeTracking | undefined;
}
