/**
 * In-memory implementation of personal time tracking (`Backend.time`),
 * matching the server rules from `time-t2-t1`:
 * - Single active timer per user with atomic switch (`stopped` returned when visible).
 * - Inclusive `[from, to]` filtering on `startedAt`, ordered `startedAt DESC, id DESC`.
 * - Full-filter `summary` across all pages; active timer counted with `0` ms.
 * - Validation for `kind`, `note` (max 1000 Unicode chars, no U+0000),
 *   timestamp bounds, `endedAt < startedAt`, and rejecting `null`/re-opening on PATCH.
 * - Privacy & session wall parity: templates & chapters rejected (`404`),
 *   closed own-space entries omitted on reads and refused with `401` on writes.
 */
import {
  TIME_ENTRY_KINDS,
  TIME_MAX_TIMESTAMP,
  TIME_NOTE_MAX_CHARS,
  type TimeEntryKind,
} from '../../logic/time.ts';
import { BackendError, refusedMember } from '../problems.ts';
import type {
  CreateTimeEntryRequest,
  ListTimeEntriesQuery,
  StartTimeEntryRequest,
  StartedTimeEntry,
  TimeEntry,
  TimeEntryKindTotals,
  TimeEntryList,
  TimeEntryPatch,
  TimeTargetKind,
  TimeTracking,
} from '../time-types.ts';

import { reads } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import { checkWorkspace, ownSpaceOf } from './spaces.ts';
import type { World } from './world.ts';

export interface TimeHost {
  readonly world: World;
  readonly userId: string;
}

interface ResolvedTarget {
  workspaceId: string | null;
  containerId: string | null;
  blockId: string | null;
  targetKind: TimeTargetKind | null;
  targetTitle: string | null;
}

const DEFAULT_PER_PAGE = 50;
const MAX_PER_PAGE = 200;

function checkKind(kind: unknown): TimeEntryKind {
  if (typeof kind === 'string' && (TIME_ENTRY_KINDS as readonly string[]).includes(kind)) {
    return kind as TimeEntryKind;
  }
  throw new BackendError(400, {
    detail: 'invalid time entry kind',
    errors: [refusedMember('kind', 'must be lecture, study, work, or other', 'invalid')],
  });
}

function checkNote(note: unknown): string {
  if (typeof note !== 'string') {
    throw new BackendError(400, {
      detail: 'invalid note',
      errors: [refusedMember('note', 'must be a string', 'invalid')],
    });
  }
  if (note.includes('\0')) {
    throw new BackendError(400, {
      detail: 'note contains NUL',
      errors: [refusedMember('note', 'must not contain NUL characters', 'invalid')],
    });
  }
  if (Array.from(note).length > TIME_NOTE_MAX_CHARS) {
    throw new BackendError(400, {
      detail: 'note too long',
      errors: [refusedMember('note', `at most ${TIME_NOTE_MAX_CHARS} characters`, 'too_long')],
    });
  }
  return note;
}

function checkTimestamp(value: unknown, member: 'startedAt' | 'endedAt'): number {
  if (
    typeof value !== 'number' ||
    !Number.isInteger(value) ||
    value < 0 ||
    value > TIME_MAX_TIMESTAMP
  ) {
    throw new BackendError(400, {
      detail: `invalid ${member}`,
      errors: [refusedMember(member, 'must be a valid epoch millisecond timestamp', 'invalid')],
    });
  }
  return value;
}

function checkOwnSpaceOpen(world: World, userId: string): void {
  const own = ownSpaceOf(world, userId);
  if (own.passwordRequired === true) {
    throw new BackendError(401, {
      reason: 'password_sign_in_required',
      detail: 'the own space opens to a session made with the password',
    });
  }
}

function isEntryVisible(world: World, userId: string, entry: TimeEntry): boolean {
  const own = ownSpaceOf(world, userId);
  if (own.passwordRequired === true) {
    if (entry.workspaceId === null || entry.workspaceId === own.id) return false;
  }
  return true;
}

function resolveTarget(
  world: World,
  userId: string,
  input: { workspaceId?: string | null; containerId?: string | null; blockId?: string | null },
): ResolvedTarget {
  const workspaceId = input.workspaceId ?? null;
  const containerId = input.containerId ?? null;
  const blockId = input.blockId ?? null;

  if (blockId !== null) {
    const block = world.blocks.get(blockId);
    if (!block) fail(404, 'no such target');
    if (containerId !== null && block.containerId !== containerId) fail(404, 'no such target');
    const container = world.containers.get(block.containerId);
    if (!container || container.isTemplate) fail(404, 'no such target');
    if (workspaceId !== null && container.workspaceId !== workspaceId) fail(404, 'no such target');
    checkWorkspace(world, userId, container.workspaceId);
    if (!reads(world, userId, container.id)) fail(404, 'no such target');
    if (block.type === 'card') {
      if ((container.kind ?? 'board') !== 'board') fail(404, 'no such target');
      if (block.fields.isTemplate === true) fail(404, 'cannot track time on a template card');
      return {
        workspaceId: container.workspaceId,
        containerId: container.id,
        blockId: block.id,
        targetKind: 'card',
        targetTitle: block.title,
      };
    }
    if (block.type === 'unit' && container.kind === 'course') {
      return {
        workspaceId: container.workspaceId,
        containerId: container.id,
        blockId: block.id,
        targetKind: 'unit',
        targetTitle: block.title,
      };
    }
    fail(404, 'unsupported target block');
  }

  if (containerId !== null) {
    const container = world.containers.get(containerId);
    if (!container || container.isTemplate) fail(404, 'no such target');
    if (workspaceId !== null && container.workspaceId !== workspaceId) fail(404, 'no such target');
    checkWorkspace(world, userId, container.workspaceId);
    if (!reads(world, userId, container.id)) fail(404, 'no such target');
    const kind = container.kind ?? 'board';
    if (kind !== 'board' && kind !== 'course') fail(404, 'unsupported target container');
    return {
      workspaceId: container.workspaceId,
      containerId: container.id,
      blockId: null,
      targetKind: kind,
      targetTitle: container.title,
    };
  }

  if (workspaceId !== null) {
    checkWorkspace(world, userId, workspaceId);
    return {
      workspaceId,
      containerId: null,
      blockId: null,
      targetKind: null,
      targetTitle: null,
    };
  }

  checkOwnSpaceOpen(world, userId);
  return {
    workspaceId: null,
    containerId: null,
    blockId: null,
    targetKind: null,
    targetTitle: null,
  };
}

function newestFirst(a: TimeEntry, b: TimeEntry): number {
  return b.startedAt - a.startedAt || (b.id < a.id ? -1 : b.id > a.id ? 1 : 0);
}

export class MemoryTime implements TimeTracking {
  readonly #host: TimeHost;
  readonly #enter: () => Promise<void>;

  constructor(host: TimeHost, enter: () => Promise<void>) {
    this.#host = host;
    this.#enter = enter;
  }

  #list(): TimeEntry[] {
    let entries = this.#host.world.timeEntries.get(this.#host.userId);
    if (!entries) {
      entries = [];
      this.#host.world.timeEntries.set(this.#host.userId, entries);
    }
    return entries;
  }

  async getActiveTimeEntry(): Promise<TimeEntry | null> {
    await this.#enter();
    const { world, userId } = this.#host;
    const running = this.#list().find(
      (entry) => entry.endedAt === null && isEntryVisible(world, userId, entry),
    );
    return running ? copy(running) : null;
  }

  async startTimeEntry(request: StartTimeEntryRequest): Promise<StartedTimeEntry> {
    await this.#enter();
    const { world, userId } = this.#host;
    const kind = checkKind(request.kind);
    const note = request.note === undefined ? '' : checkNote(request.note);
    const target = resolveTarget(world, userId, request);
    const now = world.now();
    const entries = this.#list();

    let stopped: TimeEntry | undefined;
    const previous = entries.find((entry) => entry.endedAt === null);
    if (previous) {
      previous.endedAt = Math.max(now, previous.startedAt);
      previous.updatedAt = now;
      if (isEntryVisible(world, userId, previous)) stopped = copy(previous);
    }

    const created: TimeEntry = {
      id: world.newId(),
      ...target,
      kind,
      startedAt: now,
      endedAt: null,
      note,
      createdAt: now,
      updatedAt: now,
    };
    entries.push(created);
    return stopped ? { entry: copy(created), stopped } : { entry: copy(created) };
  }

  async stopTimeEntry(): Promise<TimeEntry> {
    await this.#enter();
    const { world, userId } = this.#host;
    const running = this.#list().find(
      (entry) => entry.endedAt === null && isEntryVisible(world, userId, entry),
    );
    if (!running) fail(404, 'no active timer');
    const now = world.now();
    running.endedAt = Math.max(now, running.startedAt);
    running.updatedAt = now;
    return copy(running);
  }

  async listTimeEntries(query: ListTimeEntriesQuery = {}): Promise<TimeEntryList> {
    await this.#enter();
    const { world, userId } = this.#host;
    const matching = this.#list()
      .filter((entry) => {
        if (!isEntryVisible(world, userId, entry)) return false;
        if (query.from !== undefined && entry.startedAt < query.from) return false;
        if (query.to !== undefined && entry.startedAt > query.to) return false;
        if (query.kind !== undefined && entry.kind !== query.kind) return false;
        if (query.blockId !== undefined && entry.blockId !== query.blockId) return false;
        if (query.containerId !== undefined && entry.containerId !== query.containerId)
          return false;
        return true;
      })
      .sort(newestFirst);

    const byKind: TimeEntryKindTotals = { lecture: 0, study: 0, work: 0, other: 0 };
    let totalMs = 0;
    for (const entry of matching) {
      if (entry.endedAt !== null) {
        const ms = Math.max(0, entry.endedAt - entry.startedAt);
        totalMs += ms;
        byKind[entry.kind] += ms;
      }
    }

    const page = Math.max(0, Math.floor(query.page ?? 0));
    const perPage = Math.min(
      MAX_PER_PAGE,
      Math.max(1, Math.floor(query.perPage ?? DEFAULT_PER_PAGE)),
    );
    const start = page * perPage;
    return {
      entries: matching.slice(start, start + perPage).map(copy),
      summary: { totalMs, byKind, count: matching.length },
    };
  }

  async createTimeEntry(request: CreateTimeEntryRequest): Promise<TimeEntry> {
    await this.#enter();
    const { world, userId } = this.#host;
    const kind = checkKind(request.kind);
    const note = request.note === undefined ? '' : checkNote(request.note);
    const startedAt = checkTimestamp(request.startedAt, 'startedAt');
    const endedAt = checkTimestamp(request.endedAt, 'endedAt');
    if (endedAt < startedAt) {
      throw new BackendError(400, {
        detail: 'endedAt must be greater than or equal to startedAt',
        errors: [refusedMember('endedAt', 'must not be before startedAt', 'invalid')],
      });
    }
    const target = resolveTarget(world, userId, request);
    const now = world.now();
    const created: TimeEntry = {
      id: world.newId(),
      ...target,
      kind,
      startedAt,
      endedAt,
      note,
      createdAt: now,
      updatedAt: now,
    };
    this.#list().push(created);
    return copy(created);
  }

  async patchTimeEntry(entryId: string, patch: TimeEntryPatch): Promise<TimeEntry> {
    await this.#enter();
    const { world, userId } = this.#host;
    const entry = this.#list().find(
      (candidate) => candidate.id === entryId && isEntryVisible(world, userId, candidate),
    );
    if (!entry) fail(404, 'no such time entry');

    const raw = patch as Record<string, unknown>;
    for (const field of ['kind', 'startedAt', 'endedAt', 'note'] as const) {
      if (raw[field] === null) {
        throw new BackendError(400, {
          detail: `${field} cannot be null`,
          errors: [refusedMember(field, 'must not be null', 'invalid')],
        });
      }
    }

    const nextKind = patch.kind !== undefined ? checkKind(patch.kind) : entry.kind;
    const nextNote = patch.note !== undefined ? checkNote(patch.note) : entry.note;
    const nextStartedAt =
      patch.startedAt !== undefined
        ? checkTimestamp(patch.startedAt, 'startedAt')
        : entry.startedAt;
    const nextEndedAt =
      patch.endedAt !== undefined ? checkTimestamp(patch.endedAt, 'endedAt') : entry.endedAt;

    if (nextEndedAt !== null && nextEndedAt < nextStartedAt) {
      const pointer =
        patch.endedAt === undefined && patch.startedAt !== undefined ? 'startedAt' : 'endedAt';
      throw new BackendError(400, {
        detail: 'endedAt must be greater than or equal to startedAt',
        errors: [refusedMember(pointer, 'invalid time range', 'invalid')],
      });
    }

    entry.kind = nextKind;
    entry.note = nextNote;
    entry.startedAt = nextStartedAt;
    entry.endedAt = nextEndedAt;
    entry.updatedAt = world.now();
    return copy(entry);
  }

  async deleteTimeEntry(entryId: string): Promise<void> {
    await this.#enter();
    const { world, userId } = this.#host;
    const entries = this.#list();
    const index = entries.findIndex(
      (candidate) => candidate.id === entryId && isEntryVisible(world, userId, candidate),
    );
    if (index === -1) fail(404, 'no such time entry');
    entries.splice(index, 1);
  }
}
