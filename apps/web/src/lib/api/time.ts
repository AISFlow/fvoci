/**
 * Server API client for personal time tracking (`/api/v1/users/me/time/*`).
 * Uses the OpenAPI-generated schemas from `contract.ts` directly.
 */
import type {
  CreateTimeEntryRequest,
  ListTimeEntriesQuery,
  StartTimeEntryRequest,
  StartedTimeEntry,
  TimeEntry,
  TimeEntryList,
  TimeEntryPatch,
  TimeTracking,
} from '../state/time-types.ts';

import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

/** `GET /api/v1/users/me/time/active` */
export async function getActiveTimeEntry(http: Http): Promise<TimeEntry | null> {
  const result = await http.request<Schemas['ActiveTimeEntry']>('/api/v1/users/me/time/active');
  return result.entry;
}

/** `POST /api/v1/users/me/time/start` */
export async function startTimeEntry(
  http: Http,
  request: StartTimeEntryRequest,
): Promise<StartedTimeEntry> {
  const body: Schemas['StartTimeEntryRequest'] = {
    kind: request.kind,
    ...(request.workspaceId !== undefined ? { workspaceId: request.workspaceId } : {}),
    ...(request.containerId !== undefined ? { containerId: request.containerId } : {}),
    ...(request.blockId !== undefined ? { blockId: request.blockId } : {}),
    ...(request.note !== undefined ? { note: request.note } : {}),
  };
  return http.request<Schemas['StartedTimeEntry']>(
    '/api/v1/users/me/time/start',
    {},
    { method: 'POST', json: body },
  );
}

/** `POST /api/v1/users/me/time/stop` */
export async function stopTimeEntry(http: Http): Promise<TimeEntry> {
  return http.request<Schemas['TimeEntry']>('/api/v1/users/me/time/stop', {}, { method: 'POST' });
}

/** `GET /api/v1/users/me/time/entries` */
export async function listTimeEntries(
  http: Http,
  query: ListTimeEntriesQuery = {},
): Promise<TimeEntryList> {
  return http.request<Schemas['TimeEntryList']>(
    '/api/v1/users/me/time/entries',
    {},
    {
      query: {
        from: query.from,
        to: query.to,
        kind: query.kind,
        blockId: query.blockId,
        containerId: query.containerId,
        page: query.page,
        perPage: query.perPage,
      },
    },
  );
}

/** `POST /api/v1/users/me/time/entries` */
export async function createTimeEntry(
  http: Http,
  request: CreateTimeEntryRequest,
): Promise<TimeEntry> {
  const body: Schemas['CreateTimeEntryRequest'] = {
    kind: request.kind,
    startedAt: request.startedAt,
    endedAt: request.endedAt,
    ...(request.workspaceId !== undefined ? { workspaceId: request.workspaceId } : {}),
    ...(request.containerId !== undefined ? { containerId: request.containerId } : {}),
    ...(request.blockId !== undefined ? { blockId: request.blockId } : {}),
    ...(request.note !== undefined ? { note: request.note } : {}),
  };
  return http.request<Schemas['TimeEntry']>(
    '/api/v1/users/me/time/entries',
    {},
    { method: 'POST', json: body },
  );
}

/** `PATCH /api/v1/users/me/time/entries/{entryId}` */
export async function patchTimeEntry(
  http: Http,
  entryId: string,
  patch: TimeEntryPatch,
): Promise<TimeEntry> {
  const body: Schemas['TimeEntryPatch'] = {
    ...(patch.kind !== undefined ? { kind: patch.kind } : {}),
    ...(patch.startedAt !== undefined ? { startedAt: patch.startedAt } : {}),
    ...(patch.endedAt !== undefined ? { endedAt: patch.endedAt } : {}),
    ...(patch.note !== undefined ? { note: patch.note } : {}),
  };
  return http.request<Schemas['TimeEntry']>(
    '/api/v1/users/me/time/entries/{entryId}',
    { entryId },
    { method: 'PATCH', json: body },
  );
}

/** `DELETE /api/v1/users/me/time/entries/{entryId}` */
export async function deleteTimeEntry(http: Http, entryId: string): Promise<void> {
  await http.request<undefined>(
    '/api/v1/users/me/time/entries/{entryId}',
    { entryId },
    { method: 'DELETE' },
  );
}

export class ServerTime implements TimeTracking {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  getActiveTimeEntry(): Promise<TimeEntry | null> {
    return getActiveTimeEntry(this.#http);
  }

  startTimeEntry(request: StartTimeEntryRequest): Promise<StartedTimeEntry> {
    return startTimeEntry(this.#http, request);
  }

  stopTimeEntry(): Promise<TimeEntry> {
    return stopTimeEntry(this.#http);
  }

  listTimeEntries(query?: ListTimeEntriesQuery): Promise<TimeEntryList> {
    return listTimeEntries(this.#http, query);
  }

  createTimeEntry(request: CreateTimeEntryRequest): Promise<TimeEntry> {
    return createTimeEntry(this.#http, request);
  }

  patchTimeEntry(entryId: string, patch: TimeEntryPatch): Promise<TimeEntry> {
    return patchTimeEntry(this.#http, entryId, patch);
  }

  deleteTimeEntry(entryId: string): Promise<void> {
    return deleteTimeEntry(this.#http, entryId);
  }
}
