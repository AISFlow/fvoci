// Assertions over existing CLI/API observations, never archive/native parsing.
// Callers own the actual source/restore, authenticated requests and DB evidence.
import { expect } from "@playwright/test";
import { z } from "zod";

export const ZOTERO_FIXTURE_KEY = "SYNTHETIC_ONLY_NEVER_A_ZOTERO_KEY";
export const ZOTERO_USER_LIBRARY = {
  libraryType: "user",
  remoteLibraryId: "42",
  libraryUrl: "https://www.zotero.org/users/42",
} as const;

export interface ZoteroArchiveIds {
  connectorId: string;
  referenceId: string;
  taskId: string;
  originDocumentId: string;
  projectId: string;
  revisionIds: readonly [string, string];
  destinationUserId: string;
  destinationWorkspaceId: string;
}

const connectorSchema = z
  .object({
    id: z.string().uuid(),
    state: z.string(),
    generation: z.string(),
    completedVersion: z.string(),
    progressVersion: z.string().nullable(),
    committedPages: z.number().int(),
    retryAt: z.string().nullable(),
    reconciliationRequired: z.boolean(),
  })
  .passthrough();
const referenceSchema = z
  .object({
    id: z.string().uuid(),
    connectorId: z.string().uuid(),
    documentDisplayId: z.string(),
    itemKey: z.string(),
    remoteVersion: z.string(),
    bibliography: z.unknown(),
    returnUrl: z.string(),
    availability: z.string(),
    collectionKeys: z.array(z.string()),
    links: z.array(
      z.object({
        taskId: z.string().uuid().nullable(),
        documentId: z.string().uuid().nullable(),
        anchor: z.string().nullable(),
        displayId: z.string(),
      }),
    ),
  })
  .passthrough();
const librarySchema = z.object({
  connector: connectorSchema,
  references: z.array(referenceSchema),
  collections: z.array(
    z.object({
      key: z.string(),
      remoteVersion: z.string(),
      name: z.string(),
      parentKey: z.string().nullable(),
      availability: z.string(),
    }),
  ),
});
const observerSchema = z
  .object({
    restrictedRole: z.literal(true),
    rows: z.array(
      z
        .object({
          id: z.string().uuid(),
          documentId: z.string().uuid(),
          itemKey: z.string(),
          title: z.string(),
          availability: z.string(),
          completedVersion: z.string(),
          edges: z.number().int().nonnegative(),
        })
        .strict(),
    ),
    connector: connectorSchema.extend({ hasReadLease: z.boolean() }).strict(),
    credentialRows: z.number().int().nonnegative(),
  })
  .strict();
const requestsSchema = z.object({ requests: z.array(z.tuple([z.string(), z.string()])) }).strict();
const BIBLIOGRAPHY = {
  itemType: "book",
  title: "합성 연구 자료 🙂",
  fields: { date: "2026", publisher: "Synthetic Press", ISBN: "9780000000000" },
  creators: [
    { creatorType: "author", firstName: "민", lastName: "김" },
    { creatorType: "editor", name: "Synthetic Research Group" },
  ],
  tags: [{ tag: "연구", type: 0 }],
  relations: {},
};
const COLLECTIONS = [
  {
    key: "BCDE3456",
    remoteVersion: "6",
    name: "Study",
    parentKey: null,
    availability: "available",
  },
  {
    key: "CDEF4567",
    remoteVersion: "7",
    name: "Evidence",
    parentKey: "BCDE3456",
    availability: "available",
  },
];

function expectCompleted(connector: z.infer<typeof connectorSchema>, version: "99" | "12") {
  expect(connector).toMatchObject({
    completedVersion: version,
    progressVersion: null,
    committedPages: 0,
    retryAt: null,
  });
}
function expectReference(reference: z.infer<typeof referenceSchema>, id?: string) {
  expect(reference).toMatchObject({
    ...(id ? { id } : {}),
    itemKey: "ABCD2345",
    remoteVersion: "7",
    availability: "available",
    returnUrl: "https://www.zotero.org/users/42/items/ABCD2345",
    collectionKeys: ["BCDE3456", "CDEF4567"],
  });
  expect(reference.bibliography).toEqual(BIBLIOGRAPHY);
}

export function expectMode22Import(syncResponse: unknown) {
  const library = librarySchema.parse(syncResponse);
  expectCompleted(library.connector, "99");
  expect(library.connector).toMatchObject({
    ...ZOTERO_USER_LIBRARY,
    state: "connected",
    generation: "1",
    reconciliationRequired: false,
  });
  expect(library.references).toHaveLength(1);
  const reference = library.references[0];
  if (!reference) throw new Error("Mode22 reference missing");
  expectReference(reference);
  expect(reference.connectorId).toBe(library.connector.id);
  expect(reference.links).toHaveLength(0);
  expect(library.collections).toEqual(COLLECTIONS);
  return library;
}

export function expectRestoredDisconnected(observeReply: unknown, ids: ZoteroArchiveIds) {
  const observer = observerSchema.parse(observeReply);
  expect(observer.connector).toEqual({
    id: ids.connectorId,
    state: "disconnected",
    generation: "1",
    completedVersion: "99",
    progressVersion: null,
    committedPages: 0,
    reconciliationRequired: true,
    retryAt: null,
    hasReadLease: false,
  });
  expect(observer.credentialRows).toBe(0);
  expect(observer.rows).toEqual([
    {
      id: ids.referenceId,
      documentId: ids.referenceId,
      itemKey: "ABCD2345",
      title: "합성 연구 자료 🙂",
      availability: "available",
      completedVersion: "99",
      edges: 1,
    },
  ]);
  return observer;
}

export function expectConnectedSourceObserver(observeReply: unknown, connectorId: string) {
  const observer = observerSchema.parse(observeReply);
  expect(observer.connector).toEqual({
    id: connectorId,
    state: "connected",
    generation: "1",
    completedVersion: "12",
    progressVersion: null,
    committedPages: 0,
    reconciliationRequired: false,
    retryAt: null,
    hasReadLease: false,
  });
  expect(observer.credentialRows).toBe(1);
  return observer;
}

// Required before a reconnect can repair or replace missing archive metadata.
export function expectRestoredLibrary(libraryReply: unknown, ids: ZoteroArchiveIds) {
  const library = librarySchema.parse(libraryReply);
  expectCompleted(library.connector, "99");
  expect(library.connector).toMatchObject({
    id: ids.connectorId,
    ...ZOTERO_USER_LIBRARY,
    state: "disconnected",
    generation: "1",
    reconciliationRequired: true,
  });
  expect(library.references).toHaveLength(1);
  const reference = library.references[0];
  if (!reference) throw new Error("Restored reference missing");
  expectReference(reference, ids.referenceId);
  expect(reference.connectorId).toBe(ids.connectorId);
  expect(reference.links).toHaveLength(1);
  expect(reference.links[0]).toMatchObject({ taskId: ids.taskId, documentId: null, anchor: null });
  expect(library.collections).toEqual(COLLECTIONS);
  return library;
}

export function expectNoUpstreamGet(requestsReply: unknown, sinceCount = 0) {
  const { requests } = requestsSchema.parse(requestsReply);
  expect(Number.isSafeInteger(sinceCount) && sinceCount >= 0 && sinceCount <= requests.length).toBe(
    true,
  );
  expect(requests.slice(sinceCount)).toHaveLength(0);
  return requests.length;
}

export function expectReconnectedSince0(
  syncResponse: unknown,
  observeReply: unknown,
  requestsReply: unknown,
  ids: ZoteroArchiveIds,
  sinceCount = 0,
) {
  const library = librarySchema.parse(syncResponse);
  expectCompleted(library.connector, "12");
  expect(library.connector).toMatchObject({
    id: ids.connectorId,
    ...ZOTERO_USER_LIBRARY,
    state: "connected",
    generation: "2",
    reconciliationRequired: false,
  });
  expect(library.references).toHaveLength(2);
  const reference = library.references.find((item) => item.itemKey === "ABCD2345");
  const added = library.references.find((item) => item.itemKey === "EFGH4567");
  if (!reference || !added) throw new Error("Reconciled literal reference set missing");
  expectReference(reference, ids.referenceId);
  expect(reference.connectorId).toBe(ids.connectorId);
  expect(reference.links).toHaveLength(1);
  expect(reference.links[0]).toMatchObject({ taskId: ids.taskId, documentId: null, anchor: null });
  expect(added.id).not.toBe(ids.referenceId);
  expect(added).toMatchObject({
    connectorId: ids.connectorId,
    remoteVersion: "7",
    availability: "available",
    returnUrl: "https://www.zotero.org/users/42/items/EFGH4567",
    collectionKeys: ["BCDE3456", "CDEF4567"],
  });
  expect(added.bibliography).toEqual({ ...BIBLIOGRAPHY, title: "Second synthetic reference" });
  expect(added.links).toHaveLength(0);
  expect(library.collections).toEqual(COLLECTIONS);
  const observer = observerSchema.parse(observeReply);
  expect(observer.connector).toEqual({
    id: ids.connectorId,
    state: "connected",
    generation: "2",
    completedVersion: "12",
    progressVersion: null,
    committedPages: 0,
    reconciliationRequired: false,
    retryAt: null,
    hasReadLease: false,
  });
  expect(observer.credentialRows).toBe(1);
  expect(observer.rows).toEqual([
    {
      id: ids.referenceId,
      documentId: ids.referenceId,
      itemKey: "ABCD2345",
      title: "합성 연구 자료 🙂",
      availability: "available",
      completedVersion: "12",
      edges: 1,
    },
    {
      id: added.id,
      documentId: added.id,
      itemKey: "EFGH4567",
      title: "Second synthetic reference",
      availability: "available",
      completedVersion: "12",
      edges: 0,
    },
  ]);
  const { requests } = requestsSchema.parse(requestsReply);
  expect(Number.isSafeInteger(sinceCount) && sinceCount >= 0 && sinceCount <= requests.length).toBe(
    true,
  );
  const cycle = requests.slice(sinceCount);
  expect(cycle.length).toBeGreaterThan(0);
  const urls = cycle.map(([method, uri]) => {
    expect(method).toBe("GET");
    expect(uri.startsWith("/users/42/")).toBe(true);
    expect(uri).not.toContain(ZOTERO_FIXTURE_KEY);
    expect(uri).not.toMatch(/\/file|\/children|example\.invalid/);
    return new URL(uri, "https://api.zotero.org");
  });
  const inventories = urls.filter(
    (url) => url.pathname === "/users/42/items" && url.searchParams.get("format") === "versions",
  );
  expect(inventories).toHaveLength(3);
  for (const url of inventories) {
    expect(url.searchParams.get("since")).toBe("0");
    expect(url.searchParams.get("includeTrashed")).toBe("1");
  }
  const deleted = urls.filter((url) => url.pathname === "/users/42/deleted");
  expect(deleted).toHaveLength(1);
  expect(deleted[0]?.searchParams.get("since")).toBe("0");
  // Collection version inventories are complete and intentionally omit since.
  expect(
    urls.filter(
      (url) =>
        url.pathname === "/users/42/collections" && url.searchParams.get("format") === "versions",
    ),
  ).toHaveLength(2);
  return library;
}

const FIRST_BODY = "My authored telescope notes. 개인 의견은 유지됩니다.";
const SECOND_BODY = "My revised telescope comparison. 복원 후에도 유지됩니다.";
function expectParagraph(value: unknown, block: string, text: string) {
  const body = z
    .object({
      type: z.literal("doc"),
      content: z.array(
        z.object({
          type: z.literal("paragraph"),
          attrs: z.object({ id: z.string() }).passthrough(),
          content: z.array(
            z.object({
              type: z.literal("text"),
              text: z.string(),
              marks: z.array(z.unknown()).optional(),
            }),
          ),
        }),
      ),
    })
    .parse(value);
  expect(body.content).toHaveLength(1);
  expect(body.content[0]?.attrs.id).toBe(block);
  expect(body.content[0]?.content).toHaveLength(1);
  expect(body.content[0]?.content[0]?.text).toBe(text);
  expect(body.content[0]?.content[0]?.marks ?? []).toHaveLength(0);
}

export function expectAuthoredContent(
  state: {
    reference: unknown;
    referenceBody: unknown;
    originBody: unknown;
    revisions: readonly unknown[];
    origins: unknown;
  },
  ids: ZoteroArchiveIds,
) {
  const meta = z.object({ id: z.string().uuid(), title: z.string() }).parse(state.reference);
  expect(meta).toEqual({ id: ids.referenceId, title: "Authored astronomy 개인 의견" });
  expectParagraph(
    z.object({ contentJson: z.unknown() }).parse(state.referenceBody).contentJson,
    "owned-commentary",
    SECOND_BODY,
  );
  expectParagraph(
    z.object({ contentJson: z.unknown() }).parse(state.originBody).contentJson,
    "authored-commentary",
    "내가 쓴 의견과 비교 기록",
  );
  expect(state.revisions).toHaveLength(2);
  expect(ids.revisionIds[0]).not.toBe(ids.revisionIds[1]);
  for (const [index, text] of [FIRST_BODY, SECOND_BODY].entries()) {
    const detail = z
      .object({
        id: z.string().uuid(),
        targetId: z.string().uuid(),
        targetKind: z.literal("document"),
        reason: z.literal("manual"),
        ySnapshot: z.string().min(1),
        contentJson: z.unknown(),
      })
      .parse(state.revisions[index]);
    expect(detail.id).toBe(ids.revisionIds[index]);
    expect(detail.targetId).toBe(ids.referenceId);
    expectParagraph(detail.contentJson, "owned-commentary", text);
  }
  const origins = z
    .object({
      items: z.array(
        z.object({
          taskId: z.string().uuid(),
          documentId: z.string().uuid(),
          taskTitle: z.string(),
        }),
      ),
    })
    .parse(state.origins);
  expect(origins.items).toEqual([
    {
      taskId: ids.taskId,
      documentId: ids.originDocumentId,
      taskTitle: "Compare my authored evidence",
    },
  ]);
}

// The caller must supply its real restricted-role full affected graph snapshots,
// including persisted native/history/origin/receipt/events, not only API JSON.
export function expectRetiredReceiptReplay(
  status: number,
  beforeGraph: unknown,
  afterGraph: unknown,
) {
  expect(status).toBe(409);
  expect(afterGraph).toEqual(beforeGraph);
}

export function expectPrivateZoteroDenied(
  meStatus: number,
  meReply: unknown,
  privateReadStatuses: readonly [number, number],
  ids: ZoteroArchiveIds,
) {
  expect(meStatus).toBe(200);
  expect(z.object({ userId: z.string().uuid() }).parse(meReply).userId).not.toBe(
    ids.destinationUserId,
  );
  // Existing /zotero and /zotero/libraries/{connectorId}, known restored IDs.
  expect(privateReadStatuses).toEqual([404, 404]);
}
