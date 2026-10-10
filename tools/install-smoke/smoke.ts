#!/usr/bin/env bun
// JSON reads, assertions and small builders for the container smokes
// (scripts/{install,backup-restore,upgrade}-smoke.sh), in place of their
// inline Python. Every check keeps the meaning of the assertion it replaces.
//
//   smoke.ts port                     free 127.0.0.1 TCP port
//   smoke.ts uuid                     random UUID v4
//   smoke.ts field JSON KEY...        value at the path (a digit KEY indexes an array)
//   smoke.ts has-item JSON ID         exit 0 present, 1 absent, 2 unreadable reply
//   smoke.ts check NAME ARG...        exit 1 with the failed condition
//   smoke.ts build NAME ARG...        prints a request body
//   smoke.ts oracle ARG...            canonical per-user read projection
//   smoke.ts object-versions          mcli --json ls --versions lines -> one line per version
//   smoke.ts zotero-keyring ROOT      synthetic keyring of the Zotero fixture
//   smoke.ts redact                   stdin -> stdout, every FVOCI_REDACT line replaced
//   smoke.ts init STATE NAME [ASSERT_LOG] / phase STATE NAME / fail STATE MESSAGE...
//            error STATE STATUS LINE COMMAND / report STATE STATUS / finish STATE STATUS
//            group TITLE / endgroup / quote (stdin)   phases and first error (below)
//
// A JSON or text argument `-` is read from stdin (so a producer's failure
// still fails the pipeline under pipefail).

import { appendFileSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

export class CheckFailed extends Error {}

export function ensure(condition: unknown, detail: unknown): asserts condition {
  if (!condition)
    throw new CheckFailed(typeof detail === "string" ? detail : JSON.stringify(detail));
}

const isObject = (value: unknown): value is JsonObject =>
  typeof value === "object" && value !== null && !Array.isArray(value);

// Python truthiness: empty containers, "", 0, false and null are false.
export function truthy(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (Array.isArray(value)) return value.length > 0;
  if (isObject(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

export function canonical(value: unknown): Json {
  if (Array.isArray(value)) return value.map(canonical);
  if (isObject(value)) {
    const out: JsonObject = {};
    for (const key of Object.keys(value).sort()) out[key] = canonical(value[key]);
    return out;
  }
  return value as Json;
}

export const deepEqual = (a: unknown, b: unknown): boolean =>
  JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));

export function get(value: unknown, ...path: (string | number)[]): Json {
  let current = value;
  for (const key of path) {
    if (typeof key === "number" || /^\d+$/.test(key)) {
      ensure(Array.isArray(current), `not an array at ${String(key)}`);
      current = current[Number(key)];
    } else {
      ensure(isObject(current), `not an object at ${key}`);
      current = current[key];
    }
    ensure(current !== undefined, `missing ${String(key)}`);
  }
  return current as Json;
}

const items = (value: unknown): Json[] => {
  const list = get(value, "items");
  ensure(Array.isArray(list), "items is not a list");
  return list;
};

const ids = (list: Json[], key = "id"): Set<Json | undefined> =>
  new Set(list.map((item) => (isObject(item) ? item[key] : undefined)));

const parse = (text: string): Json => JSON.parse(text) as Json;

// A string as itself, anything else as its JSON text.
const text = (value: Json | undefined): string =>
  typeof value === "string" ? value : JSON.stringify(value ?? null);

const lastLineJson = (text: string): Json => {
  const lines = text.trim().split("\n");
  return parse(lines[lines.length - 1] ?? "");
};

const reportLines = (text: string): Json[] => {
  const reports = text.split(/\r?\n/).filter((line) => line.startsWith("{"));
  ensure(reports.length === 1, reports);
  return reports.map(parse);
};

const commaList = (value: string): string[] => value.split(",").filter(Boolean).sort();

const sortedStrings = (value: unknown): string[] => {
  ensure(Array.isArray(value), value);
  return value.map(String).sort();
};

const counts = (text: string): Record<string, number> => {
  const out: Record<string, number> = {};
  for (const item of text.split(";")) {
    const [key, raw, ...rest] = item.split("=");
    ensure(
      key !== undefined && raw !== undefined && rest.length === 0 && /^\d+$/.test(raw),
      `bad count ${item}`,
    );
    out[key] = Number(raw);
  }
  return out;
};

const atLeast = (table: Record<string, number>, name: string, min: number): boolean =>
  table[name] !== undefined && table[name] >= min;

// Comment listing: {"items": [...]} or a bare list.
function hasComment(body: Json, id: string, text: string): boolean {
  const list = isObject(body) ? (body["items"] ?? []) : Array.isArray(body) ? body : [];
  return (
    Array.isArray(list) && list.some((c) => isObject(c) && c["id"] === id && c["body"] === text)
  );
}

type Check = (...args: string[]) => void;

export const checks: Record<string, Check> = {
  // Login/accept reply names a user.
  "user-id": (reply = "") => {
    ensure(truthy(get(parse(reply), "userId")), reply);
  },
  "doctor-convert": (report = "") => {
    const r = parse(report);
    ensure(get(r, "ok") === true, r);
    const list = get(r, "checks");
    ensure(
      Array.isArray(list) &&
        list.some((c) => isObject(c) && c["name"] === "document_convert" && c["ok"] === true),
      r,
    );
  },
  "doctor-ok": (report = "") => {
    ensure(get(parse(report), "ok") === true, report);
  },
  // Literal current index settings (src/search/meili.rs index_settings).
  "meili-settings": (settings = "") => {
    const s = parse(settings);
    ensure(isObject(s), s);
    ensure(
      deepEqual(s["searchableAttributes"], [
        "title",
        "body",
        "chosung",
        "stem",
        "bibliographyBody",
        "bibliographyChosung",
        "bibliographyStem",
      ]),
      s,
    );
    ensure(
      deepEqual(s["displayedAttributes"], [
        "id",
        "kind",
        "workspaceId",
        "projectId",
        "documentId",
        "taskId",
        "commentId",
        "attachmentId",
        "chunkNo",
        "updatedAt",
      ]),
      s,
    );
    const filterable = s["filterableAttributes"] ?? [];
    ensure(Array.isArray(filterable) && filterable.includes("resourceKey"), s);
  },
  "same-body": (got = "", expected = "") => {
    const body = parse(got);
    ensure(deepEqual(get(body, "contentJson"), get(parse(expected), "contentJson")), body);
  },
  // The member's edit changed the body and kept every top-level node of it.
  "body-extends": (beforeText = "", afterText = "") => {
    const before = get(parse(beforeText), "contentJson");
    const after = get(parse(afterText), "contentJson");
    const afterNodes = get(after, "content");
    const beforeNodes = get(before, "content");
    ensure(Array.isArray(afterNodes) && Array.isArray(beforeNodes), [before, after]);
    ensure(
      !deepEqual(after, before) &&
        beforeNodes.every((node) => afterNodes.some((n) => deepEqual(n, node))),
      [before, after],
    );
  },
  comment: (body = "", id = "", text = "") => {
    ensure(hasComment(parse(body), id, text), body);
  },
  "secrets-verified": (output = "") => {
    ensure(get(lastLineJson(output), "secretsVerified") === true, lastLineJson(output));
  },
  task: (taskText = "", id = "", title = "") => {
    const task = parse(taskText);
    ensure(isObject(task) && task["title"] === title && task["id"] === id, task);
  },
  "layers-plus-one": (baseText = "", fixtureText = "") => {
    const base = parse(baseText);
    const fixture = parse(fixtureText);
    ensure(Array.isArray(base) && Array.isArray(fixture), [baseText, fixtureText]);
    ensure(deepEqual(fixture.slice(0, base.length), base) && fixture.length === base.length + 1, [
      base.length,
      fixture.length,
    ]);
  },
  // Same-ID MOVE reply: destination and ids as requested, not a replay.
  moved: (reply = "", workspaceId = "", projectId = "", documentId = "", taskId = "") => {
    const moved = parse(reply);
    const want = { workspaceId, projectId, documentId, taskId };
    ensure(isObject(moved), moved);
    for (const [key, value] of Object.entries(want)) ensure(moved[key] === value, moved);
    ensure(moved["replayed"] === false, moved);
  },
  "zf-ok": (reply = "") => {
    ensure(deepEqual(parse(reply), { ok: true }), reply);
  },
  "zf-sync": (libraryText = "") => {
    const library = parse(libraryText);
    const connector = get(library, "connector");
    ensure(
      isObject(connector) &&
        connector["state"] === "connected" &&
        connector["generation"] === "1" &&
        connector["completedVersion"] === "99",
      connector,
    );
    const references = get(library, "references");
    ensure(
      Array.isArray(references) && references.length === 1 && truthy(get(library, "collections")),
      library,
    );
  },
  "zf-observe": (reply = "") => {
    const seen = parse(reply);
    ensure(
      get(seen, "restrictedRole") === true &&
        get(seen, "credentialRows") === 1 &&
        get(seen, "connector", "state") === "connected",
      seen,
    );
    const rows = get(seen, "rows");
    ensure(Array.isArray(rows) && rows.length === 1, rows);
  },
  "zf-requests": (reply = "") => {
    ensure(truthy(get(parse(reply), "requests")), "no synthetic upstream request");
  },
  "zf-stopped": (reply = "") => {
    ensure(deepEqual(parse(reply), { stopped: true, ownedResources: 0 }), reply);
  },
  // Per-user source reads: grants, HID visibility, member-created revisions.
  "source-reads": (
    ownerText = "",
    memberText = "",
    prv = "",
    hid = "",
    memberId = "",
    memberTask = "",
    documentId = "",
  ) => {
    const owner = parse(ownerText);
    const member = parse(memberText);
    ensure(get(owner, "hidTaskStatus") === "200" && get(member, "hidTaskStatus") === "404", [
      get(owner, "hidTaskStatus"),
      get(member, "hidTaskStatus"),
    ]);
    const projectIds = (view: Json): Json[] => {
      const list = get(view, "projects");
      ensure(Array.isArray(list), list);
      return list.map((p) => get(p, 0));
    };
    ensure(
      projectIds(member).includes(prv) && !projectIds(member).includes(hid),
      get(member, "projects"),
    );
    ensure(projectIds(owner).includes(hid), get(owner, "projects"));
    ensure(
      items(get(owner, "comments")).length === 2 && items(get(owner, "revisions")).length >= 2,
      "owner comments/revisions",
    );
    for (const view of [owner, member]) {
      for (const [key, kind, target] of [
        ["memberTaskRevision", "task", memberTask],
        ["memberDocumentRevision", "document", documentId],
      ] as const) {
        const rev = get(view, key);
        ensure(
          isObject(rev) &&
            rev["createdBy"] === memberId &&
            rev["targetKind"] === kind &&
            rev["targetId"] === target,
          rev,
        );
        ensure(isObject(rev["contentJson"]) && truthy(rev["ySnapshot"]), key);
      }
      ensure(
        ids(items(get(view, "taskRevisions"))).has(get(view, "memberTaskRevision", "id")),
        get(view, "taskRevisions"),
      );
    }
  },
  // Per-user current-model reads: moved graph, own timers, wiki values and
  // view privacy, owner-only Zotero mirror.
  "source-models": (
    ownerText = "",
    memberText = "",
    movedDoc = "",
    movedTask = "",
    attachment = "",
    refDoc = "",
    ownerRun = "",
    memberRun = "",
    shared = "",
    priv = "",
    connector = "",
    textField = "",
    wikiText = "",
  ) => {
    const owner = parse(ownerText);
    const member = parse(memberText);
    const froms = (listing: Json) => new Set(items(listing).map((item) => get(item, "from", "id")));
    const runs = (history: Json) => ids(items(history), "runId");
    for (const view of [owner, member]) {
      const status = get(view, "personalMovedTaskStatus");
      ensure(
        get(view, "movedTask", "id") === movedTask && (status === "403" || status === "404"),
        status,
      );
      const body = JSON.stringify(get(view, "movedBody", "contentJson"));
      ensure(
        body.includes(movedTask) && body.includes(attachment),
        "moved body lost its task mention or file",
      );
      const movedFroms = froms(get(view, "movedBacklinks"));
      ensure(movedFroms.has(movedDoc) && movedFroms.has(refDoc), get(view, "movedBacklinks"));
      ensure(
        froms(get(view, "documentBacklinks")).has(refDoc) &&
          froms(get(view, "memberTaskBacklinks")).has(refDoc),
        "backlinks",
      );
      const movedRevisions = get(view, "movedRevisions");
      ensure(
        Array.isArray(movedRevisions) && movedRevisions.every((r) => truthy(get(r, "items"))),
        "moved document/task revisions missing",
      );
      const wikiItems = get(view, "wikiItems");
      ensure(
        Array.isArray(wikiItems) &&
          wikiItems.length === 1 &&
          items(get(view, "wikiFields")).length === 3,
        [wikiItems, get(view, "wikiFields")],
      );
      ensure(
        deepEqual(get(wikiItems, 0, "values", textField), { text: wikiText }),
        get(wikiItems, 0, "values"),
      );
    }
    ensure(
      runs(get(owner, "movedTimerHistory")).has(ownerRun) &&
        !runs(get(member, "movedTimerHistory")).has(ownerRun),
      "owner run",
    );
    ensure(
      runs(get(member, "memberTaskTimerHistory")).has(memberRun) &&
        !runs(get(owner, "memberTaskTimerHistory")).has(memberRun),
      "member run",
    );
    const ownerViews = ids(items(get(owner, "wikiViews")));
    const memberViews = ids(items(get(member, "wikiViews")));
    ensure(
      ownerViews.has(shared) &&
        !ownerViews.has(priv) &&
        memberViews.has(shared) &&
        memberViews.has(priv),
      [[...ownerViews], [...memberViews]],
    );
    const memberZotero = get(member, "zotero");
    ensure(memberZotero === "403" || memberZotero === "404", memberZotero);
    const ownerZotero = get(owner, "zotero");
    if (connector) {
      const listing = get(ownerZotero, 0);
      const library = get(ownerZotero, 1);
      const connectors = get(listing, "connectors");
      ensure(
        Array.isArray(connectors) &&
          deepEqual(
            connectors.map((c) => get(c, "id")),
            [connector],
          ) &&
          get(library, "connector", "state") === "connected",
        ownerZotero,
      );
      const references = get(library, "references");
      ensure(Array.isArray(references) && references.length === 1, library);
    } else {
      ensure(deepEqual(get(ownerZotero, "connectors"), []), ownerZotero);
    }
  },
  // Backup dir: exactly the three files, search omitted, no extra secrets,
  // key fingerprints only for ENC_ID and never the raw key (ENC_ID, ENC_K1 env).
  "backup-manifest": (manifestPath = "") => {
    const names = readdirSync(dirname(manifestPath)).sort();
    ensure(deepEqual(names, ["database.dump", "manifest.json", "storage.tar"]), names);
    const blob = readFileSync(manifestPath, "utf8");
    const manifest = parse(blob);
    ensure(get(manifest, "search", "included") === false, manifest);
    const text = JSON.stringify(manifest);
    for (const secret of ["PASSWORD_PEPPER", "MEILI_MASTER", "FVOCI_APP_PASSWORD"])
      ensure(!text.includes(secret), `${secret} in manifest`);
    const keys = get(manifest, "encryptionKeys");
    ensure(get(keys, "configured") === true, keys);
    const fingerprints = get(keys, "keyFingerprints");
    const fingerprintIds = isObject(fingerprints)
      ? Object.keys(fingerprints).sort()
      : sortedStrings(fingerprints);
    ensure(deepEqual(fingerprintIds, [process.env["ENC_ID"]]), keys);
    const k1 = process.env["ENC_K1"];
    ensure(k1 && !text.includes(k1), "raw ENCRYPTION_KEYS key in manifest");
  },
  // Nonempty source witness: the collab-saved wiki body and member task body left native rows.
  "native-counts": (text = "") => {
    const c = counts(text);
    for (const name of [
      "document_states",
      "document_collab_updates",
      "task_states",
      "task_collab_updates",
    ])
      ensure(c[name] !== undefined, c);
    const n = (name: string): number => c[name] ?? 0;
    ensure(
      n("document_states") + n("document_collab_updates") >= 1 &&
        n("task_states") + n("task_collab_updates") >= 1,
      c,
    );
  },
  "model-counts": (text = "", zotero = "") => {
    const c = counts(text);
    const nonempty = [
      "documents",
      "personal_transfer_commands",
      "task_origins",
      "time_entries",
      "task_timer_runs",
      "task_timer_segments",
      "task_timer_commands",
      "task_timer_audit",
      "collections",
      "collection_items",
      "collection_options",
      "collection_choices",
      "collection_values",
      "collection_views",
    ];
    if (zotero)
      nonempty.push(
        "zotero_connectors",
        "zotero_credentials",
        "zotero_references",
        "zotero_collections",
      );
    ensure(
      nonempty.every((table) => atLeast(c, table, 1)),
      c,
    );
    ensure(atLeast(c, "collection_views", 2) && atLeast(c, "task_timer_runs", 2), c);
  },
  "versioning-enabled": (info = "") => {
    const parsed = parse(info);
    const versioning = isObject(parsed) ? (parsed["versioning"] ?? {}) : null;
    ensure(isObject(versioning) && versioning["status"] === "Enabled", parsed);
  },
  // The single --verify-storage JSON report: exact counts, no preview damage.
  "storage-report": (output = "", checked = "", missing = "", sizeMismatch = "") => {
    const [r] = reportLines(output);
    ensure(isObject(r), r);
    const got = {
      checked: r["checked"],
      missing: sortedStrings(r["missing"]),
      sizeMismatch: sortedStrings(r["sizeMismatch"]),
    };
    const want = {
      checked: Number.parseInt(checked, 10),
      missing: commaList(missing),
      sizeMismatch: commaList(sizeMismatch),
    };
    ensure("previewMissing" in r && "previewSizeMismatch" in r, r);
    ensure(
      deepEqual(got, want) && !truthy(r["previewMissing"]) && !truthy(r["previewSizeMismatch"]),
      [got, want, r],
    );
  },
  // Wrong-key --verify-secrets: the one MFA secret is invalid, no key is unavailable.
  "mfa-invalid": (output = "") => {
    const [report] = reportLines(output);
    const mfa = get(report, "userMfa");
    const invalid = get(mfa, "invalid");
    ensure(
      get(mfa, "checked") === 1 &&
        Array.isArray(invalid) &&
        invalid.length === 1 &&
        deepEqual(get(mfa, "keyUnavailable"), []),
      mfa,
    );
  },
};

const mention = (entity: string, id: string, label: string) => ({
  type: "mention",
  attrs: { entity, id, label },
});

export const builders: Record<string, (...args: string[]) => unknown> = {
  "moved-doc-body": (task = "", attachment = "") => ({
    contentJson: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "개인 문서 본문 " }, mention("task", task, "이동 작업")],
        },
        { type: "attachment", attrs: { id: attachment, name: "이동 증빙.txt" } },
      ],
    },
  }),
  "ref-doc-body": (documentId = "", memberTask = "", movedTask = "") => ({
    contentJson: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "멤버 참조 " },
            mention("document", documentId, "백업 문서"),
            mention("task", memberTask, "멤버 작업"),
            mention("task", movedTask, "이동 작업"),
          ],
        },
      ],
    },
  }),
  "move-selection": (
    documentId = "",
    taskId = "",
    documentVersion = "",
    taskVersion = "",
    workspaceId = "",
    projectId = "",
    statusId = "",
  ) => {
    ensure(/^-?\d+$/.test(documentVersion) && /^-?\d+$/.test(taskVersion), [
      documentVersion,
      taskVersion,
    ]);
    return {
      action: "move",
      documentId,
      taskId,
      expectedDocumentVersion: Number(documentVersion),
      expectedTaskVersion: Number(taskVersion),
      destinationWorkspaceId: workspaceId,
      destinationProjectId: projectId,
      destinationStatusId: statusId,
    };
  },
};

// Canonical JSON (sorted keys, serverNow dropped in timer reads) so equal
// rows compare equal; arguments in the order user_oracle passes them.
export function oracle(args: string[]): string {
  ensure(args.length === 26, `oracle takes 26 arguments, got ${String(args.length)}`);
  const [
    workspaces,
    projects,
    hid,
    task,
    activity,
    comments,
    revisions,
    query,
    taskRevisions,
    taskRevision,
    documentRevision,
    movedDocument,
    movedBody,
    movedRevisions,
    movedTask,
    movedBacklinks,
    personalStatus,
    movedHistory,
    memberHistory,
    memberSummary,
    wikiFields,
    wikiItems,
    wikiViews,
    documentBacklinks,
    memberBacklinks,
    zotero,
  ] = args as [string, ...string[]];
  const stable = (value: Json): Json => {
    if (Array.isArray(value)) return value.map(stable);
    if (isObject(value)) {
      const out: JsonObject = {};
      for (const [key, v] of Object.entries(value)) if (key !== "serverNow") out[key] = stable(v);
      return out;
    }
    return value;
  };
  const compareTuples = (a: Json[], b: Json[]): number => {
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      const x = text(a[i]);
      const y = text(b[i]);
      if (x !== y) return x < y ? -1 : 1;
    }
    return 0;
  };
  const p = (text: string | undefined): Json => parse(text ?? "");
  return JSON.stringify(
    canonical({
      workspaces: items(p(workspaces))
        .map((w) => text(get(w, "id")))
        .sort(),
      projects: items(p(projects))
        .map((x) => ["id", "key", "name", "visibility"].map((k) => get(x, k)))
        .sort(compareTuples),
      hidTaskStatus: hid ?? "",
      memberTask: p(task),
      activity: p(activity),
      comments: p(comments),
      revisions: p(revisions),
      teamItems: get(p(query), "items"),
      taskRevisions: p(taskRevisions),
      memberTaskRevision: p(taskRevision),
      memberDocumentRevision: p(documentRevision),
      movedDocument: p(movedDocument),
      movedBody: p(movedBody),
      movedRevisions: p(movedRevisions),
      movedTask: p(movedTask),
      movedBacklinks: p(movedBacklinks),
      personalMovedTaskStatus: personalStatus ?? "",
      movedTimerHistory: stable(p(movedHistory)),
      memberTaskTimerHistory: stable(p(memberHistory)),
      memberTaskTimerSummary: stable(p(memberSummary)),
      wikiFields: p(wikiFields),
      wikiItems: get(p(wikiItems), "items"),
      wikiViews: p(wikiViews),
      documentBacklinks: p(documentBacklinks),
      memberTaskBacklinks: p(memberBacklinks),
      zotero: p(zotero),
    }),
  );
}

// Every version as `<versionId> <size> <deleteMarker> <latest> <etag>`. The
// pinned mcli emits no isLatest; the latest version is the one with the
// highest versionOrdinal, which must be unique.
export function objectVersions(text: string): string {
  const rows = text
    .split("\n")
    .filter((line) => line.trim())
    .map(parse);
  ensure(rows.length > 0 && rows.every((v) => isObject(v) && v["status"] === "success"), rows);
  const ordinals = rows.map((v) => get(v, "versionOrdinal"));
  ensure(
    ordinals.every((o) => Number.isInteger(o)) && new Set(ordinals).size === ordinals.length,
    rows,
  );
  const max = Math.max(...(ordinals as number[]));
  return rows
    .map((v) => {
      const row = v as JsonObject;
      return [
        row["versionId"],
        row["size"] ?? 0,
        truthy(row["isDeleteMarker"]),
        row["versionOrdinal"] === max,
        row["etag"] || "-",
      ]
        .map(String)
        .join(" ");
    })
    .join("\n");
}

export function zoteroKeyring(fixture: string, upstream: string): string {
  const pepper = /const PEPPER: &str =\s*r#"([\s\S]*?)"#;/.exec(fixture)?.[1];
  const apiKey = /pub const KEY: &str = "([A-Z_]+)";/.exec(upstream)?.[1];
  ensure(pepper !== undefined && apiKey !== undefined, "Zotero fixture keyring or key not found");
  const entries = Object.entries(parse(pepper) as JsonObject);
  ensure(entries.length === 1, "fixture pepper must hold exactly one key");
  const [entry] = entries;
  ensure(entry !== undefined, "fixture pepper must hold exactly one key");
  const [keyId, value] = entry;
  ensure(
    /^[a-z0-9]+$/.test(keyId) &&
      typeof value === "string" &&
      /^[0-9a-f]{64}$/.test(value) &&
      apiKey.startsWith("SYNTHETIC_ONLY_"),
    "unexpected Zotero fixture keyring",
  );
  return `${keyId} ${value} ${apiKey}`;
}

export function field(text: string, path: string[]): string {
  const value = get(parse(text), ...path);
  ensure(value !== null, `null at ${path.join(".")}`);
  return typeof value === "string" ? value : JSON.stringify(value);
}

export function hasItem(text: string, id: string): boolean {
  const body = parse(text);
  const list = isObject(body) ? (body["items"] ?? []) : [];
  return Array.isArray(list) && list.some((item) => isObject(item) && item["id"] === id);
}

// Phases and the first error, kept in a per-run state file so a `fail` inside
// a `$(...)` subshell still names the first error. In GitHub Actions a phase is
// a `::group::` block and a failure is also an `::error::` annotation;
// elsewhere they are `== phase NAME` lines. The shell keeps only the trap glue.
type PhaseState = {
  name: string;
  assertLog: string;
  phase: string;
  failure: string;
  lastError: string;
  report: string;
};

const inActions = (): boolean => process.env["GITHUB_ACTIONS"] === "true";

function readState(path: string): PhaseState {
  const raw = parse(readFileSync(path, "utf8"));
  ensure(isObject(raw), `bad phase state ${path}`);
  const field = (key: keyof PhaseState): string => (typeof raw[key] === "string" ? raw[key] : "");
  return {
    name: field("name"),
    assertLog: field("assertLog"),
    phase: field("phase"),
    failure: field("failure"),
    lastError: field("lastError"),
    report: field("report"),
  };
}

const writeState = (path: string, state: PhaseState): void => {
  writeFileSync(path, JSON.stringify(state));
};

// Annotation text is data, not a workflow command.
const escapeAnnotation = (text: string): string =>
  text.replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A");

type PhaseOutput = { out: string; err: string };

export function phaseCommand(
  command: string,
  args: string[],
  stdin: () => Promise<string>,
): Promise<PhaseOutput> | PhaseOutput {
  const [path = "", ...rest] = args;
  const lines: string[] = [];
  const errors: string[] = [];
  const group = (title: string) => lines.push(inActions() ? `::group::${title}` : `== ${title}`);
  const endgroup = () => {
    if (inActions()) lines.push("::endgroup::");
  };
  switch (command) {
    case "init":
      writeState(path, {
        name: rest[0] ?? "smoke",
        assertLog: rest[1] ?? "",
        phase: "",
        failure: "",
        lastError: "",
        report: "",
      });
      break;
    case "phase": {
      const state = readState(path);
      if (state.phase) endgroup();
      writeState(path, { ...state, phase: rest[0] ?? "", failure: "", lastError: "" });
      lines.push(inActions() ? `::group::phase ${rest[0] ?? ""}` : `== phase ${rest[0] ?? ""}`);
      break;
    }
    case "fail": {
      const state = readState(path);
      const message = rest.join(" ");
      errors.push(`FAIL: ${message}`);
      if (state.assertLog) appendFileSync(state.assertLog, `FAIL: ${message}\n`);
      writeState(path, { ...state, failure: message });
      break;
    }
    case "error": {
      // ERR trap of the main shell: STATUS LINE COMMAND (recorded, never exits).
      const [status = "", line = "", ...commandText] = rest;
      writeState(path, {
        ...readState(path),
        lastError: `line ${line}: ${commandText.join(" ")} (exit ${status})`,
      });
      break;
    }
    case "report": {
      // First call of the EXIT trap: closes the group, names the failed phase.
      const state = readState(path);
      const status = Number(rest[0] ?? "1");
      if (state.phase) endgroup();
      let report = "";
      if (status !== 0) {
        const first =
          status === 130 || status === 143
            ? `interrupted (SIG${status === 130 ? "INT" : "TERM"})`
            : state.failure || state.lastError || "see the output above";
        report = `phase ${state.phase || "setup"} failed (exit ${String(status)}): ${first}`;
        errors.push(report);
        if (inActions()) lines.push(`::error title=${state.name}::${escapeAnnotation(report)}`);
      }
      writeState(path, { ...state, report });
      break;
    }
    case "finish": {
      // Last call of the EXIT trap: the failure is the final line; a teardown
      // failure after passing checks is reported here. Removes the state file.
      const state = readState(path);
      const status = Number(rest[0] ?? "1");
      if (status !== 0) {
        let report = state.report;
        if (!report) {
          report = `phase cleanup failed (exit ${String(status)}): teardown left resources or failed`;
          if (inActions()) lines.push(`::error title=${state.name}::${escapeAnnotation(report)}`);
        }
        errors.push(`${state.name}: ${report}`);
      }
      rmSync(path, { force: true });
      break;
    }
    case "group":
      group(args.join(" "));
      break;
    case "endgroup":
      endgroup();
      break;
    case "quote":
      // Log text between the markers cannot act as a workflow command.
      return stdin().then((text) => {
        if (!inActions()) return { out: text, err: "" };
        const token = crypto.randomUUID().replaceAll("-", "");
        return {
          out: `::stop-commands::${token}\n${text}${text.endsWith("\n") || !text ? "" : "\n"}::${token}::\n`,
          err: "",
        };
      });
    default:
      throw new CheckFailed(`unknown phase command ${command}`);
  }
  // The caller prints out before err, so a closed group precedes the failure line.
  return {
    out: lines.length ? `${lines.join("\n")}\n` : "",
    err: errors.length ? `${errors.join("\n")}\n` : "",
  };
}

export const phaseCommands = new Set([
  "init",
  "phase",
  "fail",
  "error",
  "report",
  "finish",
  "group",
  "endgroup",
  "quote",
]);

function freePort(): number {
  const server = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const { port } = server;
  server.stop(true);
  return port;
}

async function main(argv: string[]): Promise<number> {
  const [first = "", ...raw] = argv;
  if (phaseCommands.has(first)) {
    const { out, err } = await phaseCommand(first, raw, () => Bun.stdin.text());
    process.stdout.write(out);
    process.stderr.write(err);
    return 0;
  }
  let stdin: string | undefined;
  const input = async (arg: string | undefined): Promise<string> => {
    if (arg !== "-") return arg ?? "";
    stdin ??= await Bun.stdin.text();
    return stdin;
  };
  const [command, ...rest] = argv;
  const args = await Promise.all(rest.map(input));
  const out = (text: string) => process.stdout.write(`${text}\n`);
  switch (command) {
    case "port":
      out(String(freePort()));
      return 0;
    case "uuid":
      out(crypto.randomUUID());
      return 0;
    case "field":
      out(field(args[0] ?? "", args.slice(1)));
      return 0;
    case "has-item":
      return hasItem(args[0] ?? "", args[1] ?? "") ? 0 : 1;
    case "oracle":
      out(oracle(args));
      return 0;
    case "object-versions":
      out(objectVersions(await Bun.stdin.text()));
      return 0;
    case "redact": {
      let text = await Bun.stdin.text();
      for (const secret of (process.env["FVOCI_REDACT"] ?? "").split("\n"))
        if (secret) text = text.replaceAll(secret, "[redacted]");
      process.stdout.write(text);
      return 0;
    }
    case "zotero-keyring": {
      const root = args[0] ?? "";
      out(
        zoteroKeyring(
          await Bun.file(`${root}/src/bin/e2e-fixture/zotero.rs`).text(),
          await Bun.file(`${root}/src/integrations/zotero.rs`).text(),
        ),
      );
      return 0;
    }
    case "check": {
      const check = checks[args[0] ?? ""];
      if (!check) break;
      check(...args.slice(1));
      return 0;
    }
    case "build": {
      const build = builders[args[0] ?? ""];
      if (!build) break;
      out(JSON.stringify(build(...args.slice(1))));
      return 0;
    }
  }
  process.stderr.write(`smoke.ts: unknown command: ${argv.join(" ")}\n`);
  return 2;
}

if (import.meta.main) {
  try {
    process.exitCode = await main(process.argv.slice(2));
  } catch (error) {
    const label = process.argv.slice(2, process.argv[2] === "check" ? 4 : 3).join(" ");
    process.stderr.write(
      `smoke.ts ${label} failed: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    // has-item keeps 1 for "absent"; a reply it cannot read is 2.
    process.exitCode = process.argv[2] === "has-item" ? 2 : 1;
  }
}
