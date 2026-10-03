import { extractText } from "@fvoci/editor/extract";
import { yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { sameValue } from "@fvoci/editor/vue";
import type * as Y from "yjs";
import { api, ensureOk } from "@/lib/api";

interface BodySchema {
  nodes: Record<string, unknown>;
  marks: Record<string, unknown>;
}
export interface ReadonlyBodyScope {
  workspaceId: string;
  targetId: string;
  kind: "wiki" | "project" | "task";
  projectId: string | null;
  actorId: string;
  credentialId: string;
  lifetime: number;
  doc: Y.Doc;
  provider: object;
  generation: number;
  connected: boolean;
  synced: boolean;
  pending: boolean;
  allowed: boolean;
  schema?: BodySchema | null;
}

function plainRecord(value: unknown): value is Record<string, unknown> {
  return (
    typeof value === "object" && value !== null && Object.getPrototypeOf(value) === Object.prototype
  );
}
function dataOnly(value: unknown, ancestors = new WeakSet()): boolean {
  if (typeof value !== "object" || value === null) return true;
  const proto: unknown = Object.getPrototypeOf(value);
  if (proto !== null && proto !== Object.prototype && proto !== Array.prototype) return true;
  if (ancestors.has(value)) return false;
  ancestors.add(value);
  try {
    return Reflect.ownKeys(value).every((key) => {
      if (typeof key !== "string") return false;
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      return (
        !!descriptor &&
        Object.hasOwn(descriptor, "value") &&
        (descriptor.enumerable || (Array.isArray(value) && key === "length")) &&
        dataOnly(descriptor.value, ancestors)
      );
    });
  } finally {
    ancestors.delete(value);
  }
}
function attrsDictionary(value: unknown): Map<string, unknown> | null {
  if (typeof value !== "object" || value === null) return null;
  const proto: unknown = Object.getPrototypeOf(value);
  if (proto !== null && proto !== Object.prototype) return null;
  const fields = new Map<string, unknown>();
  for (const key of Reflect.ownKeys(value)) {
    if (typeof key !== "string") return null;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !Object.hasOwn(descriptor, "value")) return null;
    fields.set(key, descriptor.value);
  }
  return fields;
}
function sameSupportedMark(a: unknown, b: unknown): boolean {
  if (!plainRecord(a) || !plainRecord(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length || !keys.every((key) => Object.hasOwn(b, key)))
    return false;
  return keys.every((key) => {
    if (key !== "attrs") return sameValue(a[key], b[key]);
    const left = attrsDictionary(a[key]),
      right = attrsDictionary(b[key]);
    // SDK null-prototype mark attrs and transported JSON ordinary records
    // carry the same own data. Missing/null/undefined/opaque attrs stay exact.
    if (!left || !right) return sameValue(a[key], b[key]);
    return (
      left.size === right.size &&
      [...left].every(([field, value]) => right.has(field) && sameValue(value, right.get(field)))
    );
  });
}
function uniqueSupportedMarks(value: unknown, schema: BodySchema): Map<string, unknown> | null {
  if (!Array.isArray(value)) return null;
  const byType = new Map<string, unknown>();
  for (const mark of value) {
    if (
      !plainRecord(mark) ||
      !Object.hasOwn(mark, "type") ||
      typeof mark.type !== "string" ||
      !Object.hasOwn(schema.marks, mark.type) ||
      byType.has(mark.type)
    )
      return null;
    byType.set(mark.type, mark);
  }
  return byType;
}
function pureUnmarkedText(node: unknown): string | null {
  return plainRecord(node) &&
    Object.keys(node).length === 2 &&
    Object.hasOwn(node, "type") &&
    node.type === "text" &&
    Object.hasOwn(node, "text") &&
    typeof node.text === "string" &&
    node.text.length > 0
    ? node.text
    : null;
}
function knownUnmarkedEmoji(node: unknown, schema: BodySchema): string | null {
  if (
    !Object.hasOwn(schema.nodes, "text") ||
    !Object.hasOwn(schema.nodes, "emoji") ||
    !plainRecord(node) ||
    Object.keys(node).length !== 2 ||
    !Object.hasOwn(node, "type") ||
    node.type !== "emoji" ||
    !Object.hasOwn(node, "attrs") ||
    !plainRecord(node.attrs) ||
    Object.keys(node.attrs).length !== 1 ||
    !Object.hasOwn(node.attrs, "name") ||
    typeof node.attrs.name !== "string" ||
    !node.attrs.name
  )
    return null;
  // Existing public extractor delegates this ONE strict atom to the pinned
  // Emoji registry. Its unknown-name textual fallback proves no equivalence.
  const glyph = extractText(node);
  return glyph && glyph !== `:${node.attrs.name}:` ? glyph : null;
}
function textExpansionEnd(
  text: string,
  nodes: unknown[],
  start: number,
  schema: BodySchema,
): number | null {
  let offset = 0,
    sawEmoji = false,
    previousText = false;
  for (let i = start; i < nodes.length; i++) {
    const plain = pureUnmarkedText(nodes[i]);
    const glyph = plain === null ? knownUnmarkedEmoji(nodes[i], schema) : null;
    // The installed producer splits text only around unmarked Emoji atoms.
    // Do not coalesce arbitrary adjacent text nodes or other node types.
    if ((plain !== null && previousText) || (plain === null && glyph === null)) return null;
    const segment = plain ?? glyph;
    if (!segment || !text.startsWith(segment, offset)) return null;
    offset += segment.length;
    sawEmoji ||= glyph !== null;
    previousText = plain !== null;
    if (offset === text.length) return sawEmoji ? i + 1 : null;
  }
  return null;
}
function sameContent(left: unknown[], right: unknown[], schema: BodySchema): boolean {
  if (
    ![left, right].every(
      (nodes) =>
        Object.keys(nodes).length === nodes.length &&
        Object.keys(nodes).every((key, index) => key === String(index)),
    )
  )
    return false;
  let l = 0,
    r = 0;
  while (l < left.length && r < right.length) {
    if (sameCommittedProjection(left[l], right[r], schema)) {
      l++;
      r++;
      continue;
    }
    const leftText = pureUnmarkedText(left[l]),
      rightText = pureUnmarkedText(right[r]);
    const rightEnd = leftText === null ? null : textExpansionEnd(leftText, right, r, schema);
    if (rightEnd !== null) {
      l++;
      r = rightEnd;
      continue;
    }
    const leftEnd = rightText === null ? null : textExpansionEnd(rightText, left, l, schema);
    if (leftEnd === null) return false;
    l = leftEnd;
    r++;
  }
  return l === left.length && r === right.length;
}
/** The pinned Rust projector sorts unique raw mark names, while JS preserves
 * Y.Text format-item order. Only this documented supported-node/unique-mark
 * order equivalence, plus the pinned unmarked Emoji/text segmentation above,
 * is permitted; opaque values and other arrays stay exact. */
function sameCommittedProjection(
  a: unknown,
  b: unknown,
  schema: BodySchema | null | undefined,
): boolean {
  if (sameValue(a, b)) return true;
  if (
    !schema ||
    !plainRecord(a) ||
    !plainRecord(b) ||
    typeof a.type !== "string" ||
    a.type !== b.type ||
    !Object.hasOwn(schema.nodes, a.type)
  )
    return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length || !keys.every((key) => Object.hasOwn(b, key)))
    return false;
  return keys.every((key) => {
    if (key === "marks") {
      if (sameValue(a[key], b[key])) return true;
      const left = uniqueSupportedMarks(a[key], schema),
        right = uniqueSupportedMarks(b[key], schema);
      return (
        !!left &&
        !!right &&
        left.size === right.size &&
        [...left].every(
          ([type, mark]) => right.has(type) && sameSupportedMark(mark, right.get(type)),
        )
      );
    }
    if (key === "content" && Array.isArray(a[key]) && Array.isArray(b[key])) {
      return sameContent(a[key], b[key], schema);
    }
    return sameValue(a[key], b[key]);
  });
}
type ReadBody = (scope: ReadonlyBodyScope) => Promise<unknown>;
async function readBody(scope: ReadonlyBodyScope): Promise<unknown> {
  let response: unknown;
  if (scope.kind === "task") {
    response = await ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
        params: { path: { workspace_id: scope.workspaceId, task_id: scope.targetId } },
        cache: "no-store",
      }),
    );
  } else if (scope.kind === "project") {
    if (!scope.projectId) throw new Error("missing captured project scope");
    response = await ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body",
        {
          params: {
            path: {
              workspace_id: scope.workspaceId,
              project_id: scope.projectId,
              document_id: scope.targetId,
            },
          },
          cache: "no-store",
        },
      ),
    );
  } else {
    response = await ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body", {
        params: { path: { workspace_id: scope.workspaceId, document_id: scope.targetId } },
        cache: "no-store",
      }),
    );
  }
  // Existing JSON routes return the stored projection unchanged. No opaque
  // fields are removed, and a denial never retries through another scope.
  if (typeof response !== "object" || response === null || !("contentJson" in response))
    throw new Error("committed JSON body unavailable");
  return response.contentJson;
}
function authorized(scope: ReadonlyBodyScope | null): scope is ReadonlyBodyScope {
  return (
    !!scope &&
    scope.allowed &&
    scope.connected &&
    scope.synced &&
    !scope.pending &&
    !!scope.actorId &&
    !!scope.credentialId
  );
}
function sameScope(before: ReadonlyBodyScope, after: ReadonlyBodyScope): boolean {
  return (
    before.workspaceId === after.workspaceId &&
    before.targetId === after.targetId &&
    before.kind === after.kind &&
    before.projectId === after.projectId &&
    (before.schema ?? null) === (after.schema ?? null) &&
    before.actorId === after.actorId &&
    before.credentialId === after.credentialId &&
    before.lifetime === after.lifetime &&
    before.doc === after.doc &&
    before.provider === after.provider &&
    before.generation === after.generation
  );
}

/** View-authorized copy proof, separate from the writer's persist ACK/saved flag. */
export function useReadonlyCommittedBody(
  current: () => ReadonlyBodyScope | null,
  read: ReadBody = readBody,
): () => Promise<boolean> {
  return async () => {
    const before = current();
    if (!authorized(before)) return false;
    let updates = 0;
    const changed = () => {
      updates++;
    };
    before.doc.on("update", changed);
    try {
      const live = yDocToTiptapJson(before.doc);
      const committed = await read(before);
      const after = current();
      return (
        updates === 0 &&
        authorized(after) &&
        sameScope(before, after) &&
        dataOnly(live) &&
        dataOnly(committed) &&
        sameCommittedProjection(live, committed, before.schema) &&
        sameCommittedProjection(yDocToTiptapJson(after.doc), committed, after.schema)
      );
    } catch {
      return false;
    } finally {
      before.doc.off("update", changed);
    }
  };
}
