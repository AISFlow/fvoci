// Trace summary controls (ported from the Python parts of
// scripts/fixtures/web-e2e/trace-summary-fixture-test.sh): synthetic trace.zip
// files with tokens in paths, parameters, console text and page errors, and the
// template diagnostic's exact/private/nullable inputs and rejection controls.
import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Zip, ZipDeflate } from "fflate";
import { shortUrl, summarize } from "./trace-summary.ts";

type Value = string | Uint8Array;
type Json = Record<string, unknown>;

/** Unix mode and the "made by" system (3 Unix, 0 DOS) of one record. */
type Mode = { mode: number; os: number };

/** A deflated ZIP of the given members, in order; duplicate names are kept. */
function zipOf(members: [string, Value][], modes: Record<string, Mode> = {}): Uint8Array {
  const chunks: Uint8Array[] = [];
  const zip = new Zip((error, chunk) => {
    if (error) throw error;
    chunks.push(chunk);
  });
  for (const [name, value] of members) {
    const mode = modes[name];
    const file = new ZipDeflate(name, { level: 6 });
    if (mode !== undefined) {
      // The mode goes in the high half of the external attributes.
      file.os = mode.os;
      file.attrs = mode.mode * 0x10000;
    }
    zip.add(file);
    file.push(typeof value === "string" ? new TextEncoder().encode(value) : value, true);
  }
  zip.end();
  return Buffer.concat(chunks);
}

/** Apply `edit` to every local and central record named `name`. */
function editRecords(
  archive: Uint8Array,
  name: string,
  edit: (view: DataView, at: number, central: boolean) => void,
): Uint8Array {
  const out = Uint8Array.from(archive);
  const view = new DataView(out.buffer);
  const wanted = new TextEncoder().encode(name);
  for (let at = 0; at + 46 <= out.length; at += 1) {
    const signature = view.getUint32(at, true);
    const nameAt = signature === 0x04034b50 ? 30 : signature === 0x02014b50 ? 46 : 0;
    if (nameAt === 0) continue;
    const length = view.getUint16(at + (signature === 0x04034b50 ? 26 : 28), true);
    const found = out.subarray(at + nameAt, at + nameAt + length);
    if (length === wanted.length && found.every((byte, index) => byte === wanted[index])) {
      edit(view, at, signature === 0x02014b50);
    }
  }
  return out;
}

/** Set the encryption bit of the records named `name` (no real cipher). */
const markEncrypted = (archive: Uint8Array, name: string) =>
  editRecords(archive, name, (view, at, central) => {
    const flags = at + (central ? 8 : 6);
    view.setUint16(flags, view.getUint16(flags, true) | 1, true);
  });

/** Flip one bit of the central-directory CRC-32 of the records named `name`. */
const corruptCrc = (archive: Uint8Array, name: string) =>
  editRecords(archive, name, (view, at, central) => {
    if (central) view.setUint32(at + 16, view.getUint32(at + 16, true) ^ 1, true);
  });

const cli = join(import.meta.dir, "trace-summary.ts");

describe("trace summary redaction", () => {
  test("no token reaches the summary; diagnostic paths survive; the key regex stays linear", () => {
    const share = "S".repeat(43);
    const bare = "B".repeat(45);
    const snap = (url: string, status: number, failure = "", kind = "document", t = 1.0) => ({
      type: "resource-snapshot",
      snapshot: {
        request: { method: "GET", url },
        response: { status, _failureText: failure },
        _resourceType: kind,
        startedDateTime: "2026-09-29T00:00:00.000Z",
        time: 12,
        _monotonicTime: t,
      },
    });
    const network = [
      snap(`http://127.0.0.1:4000/s/${share}?code=QUERYSECRET`, 404),
      snap("http://127.0.0.1:4000/assets/index-DiwrgTda.js", 0, "net::ERR_ABORTED", "script", 1.5),
      // Shorter than LONG_TOKEN: only the path-token rule redacts it.
      snap("http://127.0.0.1:4000/api/v1/invitations/PATHSECRET/accept", 404, "", "fetch", 1.7),
    ];
    const events = [
      {
        type: "console",
        messageType: "error",
        time: 2.0,
        text: `fetch failed access_token=PARAMSECRET1 inviteToken=PARAMSECRET2 ${bare} postgresql://u:DBSECRET@h/db`,
        location: { url: "http://127.0.0.1:4000/assets/app.js" },
      },
      {
        type: "event",
        method: "pageError",
        time: 3.0,
        params: {
          error: { error: { name: "Error", message: "a".repeat(200000) + " state=PARAMSECRET3" } },
        },
      },
    ];
    const work = mkdtempSync(join(tmpdir(), "fvoci-trace-summary."));
    try {
      const archive = join(work, "trace.zip");
      writeFileSync(
        archive,
        zipOf([
          ["0-trace.network", network.map((entry) => JSON.stringify(entry)).join("\n")],
          ["0-trace.trace", events.map((entry) => JSON.stringify(entry)).join("\n")],
        ]),
      );
      const started = performance.now();
      const result = Bun.spawnSync([process.execPath, cli, archive], {
        stdout: "pipe",
        stderr: "pipe",
        timeout: 20_000,
      });
      expect(performance.now() - started).toBeLessThan(20_000);
      expect(result.exitCode).toBe(0);
      const summary = result.stdout.toString();
      expect(summary.startsWith("browser summary: ")).toBe(true);
      for (const secret of [
        "PATHSECRET",
        "QUERYSECRET",
        "PARAMSECRET1",
        "PARAMSECRET2",
        "PARAMSECRET3",
        "DBSECRET",
        "S".repeat(20),
        "B".repeat(20),
      ]) {
        expect(summary).not.toContain(secret);
      }
      expect(summary).toContain("/s/<redacted>?…");
      expect(summary).toContain("/api/v1/invitations/<redacted>/accept");
      expect(summary).toContain("GET ERR script /assets/index-DiwrgTda.js");
      expect(summary).toContain("net::ERR_ABORTED");
      expect(summary).toContain("console   error fetch failed");
      expect(summary).toContain("pageerror");
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  });

  test("paths are redacted as Python split them: no backslash or dot-segment rewriting", () => {
    const token = "SYNTHETIC_PRIVATE";
    // Expected values are the replaced Python script's output for the same URLs.
    const expected: [string, string][] = [];
    for (const family of ["s", "invite", "share", "invitations", "ics"]) {
      const redacted = `/${family}/<redacted>`;
      expected.push(
        [`http://localhost/${family}/prefix\\${token}`, redacted],
        [`http://localhost/${family}/..\\${token}`, redacted],
        [`http://127.0.0.1:4000/${family}/%2e%2e%2f${token}`, redacted],
        [
          `https://Example.COM:8443/${family}/a%5c..\\%2E%2E${token}?q=1`,
          `https://example.com${redacted}?…`,
        ],
      );
    }
    for (const [url, path] of expected) {
      expect(shortUrl(url), url).toBe(path);
    }
    // Same rule as Python: a later segment is not a path token (parity, not a new rule).
    expect(shortUrl(`http://localhost/s/../${token}`)).toBe(`/s/<redacted>/${token}`);
    expect(shortUrl("http://[::1]:3000/s/abc")).toBe("/s/<redacted>");
    expect(shortUrl("http://user:pw@Host:99/x")).toBe("http://host/x");
    for (const url of ["http://[1.2.3.4]/p", "http://[::1/p", "http://a℀b.com/p"]) {
      expect(shortUrl(url), url).toBe("<unparsable url>");
    }
    for (const url of ["", null, 0, false]) expect(shortUrl(url)).toBe("/");

    const rows = expected.map(([url], index) =>
      JSON.stringify({
        type: "resource-snapshot",
        snapshot: {
          request: { method: "GET", url },
          response: { status: 200 },
          _monotonicTime: index,
        },
      }),
    );
    const summary = summarize(zipOf([["0-trace.network", rows.join("\n")]]));
    expect(summary).not.toContain(token);
    expect(summary.split("\n").filter((line) => line.includes("<redacted>"))).toHaveLength(20);
  });

  test("an unreadable archive fails with exit 1 and no stdout", () => {
    const work = mkdtempSync(join(tmpdir(), "fvoci-trace-summary."));
    try {
      const archive = join(work, "trace.zip");
      writeFileSync(archive, "not a zip");
      for (const args of [[archive], []]) {
        const result = Bun.spawnSync([process.execPath, cli, ...args], {
          stdout: "pipe",
          stderr: "pipe",
        });
        expect(result.exitCode).toBe(1);
        expect(result.stdout.toString()).toBe("");
      }
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  });
});

// Independently declared fixture data, not a converted production snapshot.
// The approved CI artifact already includes this stdout/browser-summary file;
// no raw ZIP, document content or arbitrary attachment becomes uploadable.
describe("template diagnostic", () => {
  const name = "w3-template-native-selection-observation.json";
  const member = "attachments/" + "a".repeat(40);
  const secret = "DIAGNOSTIC_PRIVATE_VALUE";
  const frame: Json = {
    at: 10,
    stage: "ShiftHome:original-return",
    owner: "[1,2,1,3,4,5,6]",
    bindingGeneration: 1,
    nativeId: "NATIVE_ID_PRIVATE",
    native: { inside: true, text: "한글과 😀 링크", positions: { anchor: 9, head: 1 } },
    pm: { anchor: 9, head: 1, empty: false, type: "text", marks: [{ href: secret }] },
    focus: {
      activeTag: "DIV",
      activeLabel: secret,
      editor: true,
      editorEditable: true,
      domEditable: "true",
      composing: false,
    },
    auth: {
      authenticated: true,
      synced: true,
      scope: "read-write",
      status: "connected",
      password: secret,
    },
    updates: 0,
    localUpdates: 0,
    generationUpdates: 0,
    generationLocalUpdates: 0,
    bubble: { visibility: "visible", opacity: "1" },
    dialog: false,
    pmDocument: { type: "doc", attrs: { rawFuture: secret }, text: secret },
    unknown: "https://private.example/" + secret + "?password=" + secret,
  };
  const actions = [
    "openDoc:original",
    "ShiftHome:original-return",
    "bubble:original-visible",
    "popup:original-open-focus",
    "compositionEnter:original-no-link",
    "Cancel:original-native-text",
    "Apply:original-selected-text",
    "save:original",
    "finally",
  ];
  const clone = <T>(value: T): T => structuredClone(value);
  const boundary = actions.map((stage) => ({ ...clone(frame), stage }));
  const transition = {
    at: 20,
    stage: "provider:status",
    previousOwner: frame.owner,
    owner: "[11,12,11,13,14,15,16]",
    bindingGeneration: 2,
  };
  const data = {
    frames: [frame],
    critical: boundary,
    actionBoundaries: boundary.slice(0, -1),
    ownerChanges: [transition],
    observedMismatches: [frame],
    firstObservedState: frame,
    updates: 1,
    localUpdates: 1,
    earlyBindingNotCaptured: true,
    totals: { frames: 1, critical: 9, actionBoundaries: 8, ownerChanges: 1, observedMismatches: 1 },
    dropped: {
      frames: 0,
      critical: 0,
      actionBoundaries: 0,
      ownerChanges: 0,
      observedMismatches: 0,
    },
  };
  type Data = typeof data & { caretBoundaries?: unknown };
  const reference = { name, contentType: "application/json", file: member };
  const network = {
    type: "resource-snapshot",
    snapshot: {
      request: { method: "GET", url: "http://localhost/assets/fixture.js" },
      response: { status: 200 },
      _resourceType: "script",
      time: 1,
      _monotonicTime: 1,
    },
  };
  interface Options {
    payload?: unknown;
    ref?: Json;
    test?: string;
    extra?: [string, string][];
    duplicate?: boolean;
    modes?: Record<string, Mode>;
    edit?: (archive: Uint8Array) => Uint8Array;
  }
  const run = (label: string, options: Options = {}): Json => {
    const payload = "payload" in options ? options.payload : data;
    const members: [string, Value][] = [
      ["0-trace.network", JSON.stringify(network)],
      [
        "test.trace",
        options.test ?? JSON.stringify({ type: "after", attachments: [options.ref ?? reference] }),
      ],
      [member, payload instanceof Uint8Array ? payload : JSON.stringify(payload)],
      ...(options.extra ?? []),
    ];
    if (options.duplicate) members.push([member, JSON.stringify(data)]);
    const archive = zipOf(members, options.modes);
    const output = summarize(options.edit ? options.edit(archive) : archive);
    expect(output, `${label}: base digest lost`).toContain("GET 200 script /assets/fixture.js");
    for (const forbidden of [
      secret,
      "NATIVE_ID_PRIVATE",
      "private.example",
      "한글과",
      "😀",
      "rawFuture",
      "activeLabel",
      "pmDocument",
      "password",
    ]) {
      expect(output, `${label}: private field leaked`).not.toContain(forbidden);
    }
    const prefix = "w3-template-diagnostic ";
    const records = output
      .split("\n")
      .filter((line) => line.startsWith(prefix))
      .map((line) => JSON.parse(line.slice(prefix.length)) as Json);
    expect(records, `${label}: missing diagnostic outcome`).toHaveLength(1);
    const record = records[0] ?? {};
    expect(Buffer.byteLength(JSON.stringify(record))).toBeLessThanOrEqual(49152);
    return record;
  };
  const reject = (reason: string) => ({ available: false, reason });

  test("valid input exports the allowlist only", () => {
    const valid = run("valid") as {
      available: boolean;
      counts: Json;
      actions: Record<string, Json | null>;
      firstIdentityChange: Json;
      firstMismatch: { native: Json };
      missingActions: string[];
      earlyBindingNotCaptured: boolean;
    };
    expect(valid.available).toBe(true);
    expect(valid.counts.frames).toEqual({ retained: 1, total: 1, dropped: 0, unknown: false });
    const selected = valid.actions["ShiftHome:original-return"] as Json & { nativeId: Json };
    expect(selected.native).toEqual({
      inside: true,
      codePoints: 8,
      equalsKnownFixtureCjkEmoji: true,
      positions: { anchor: 9, head: 1 },
      mappingUnknown: false,
    });
    expect(selected.pm).toEqual({ anchor: 9, head: 1, empty: false, type: "text" });
    expect(selected.auth).toEqual({
      authenticated: true,
      synced: true,
      scope: "read-write",
      status: "connected",
    });
    expect(selected.nativeId.present).toBe(true);
    expect(selected.nativeId.hash).toHaveLength(16);
    expect(valid.firstIdentityChange.previousOwner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(valid.firstIdentityChange.owner).toEqual([11, 12, 11, 13, 14, 15, 16]);
    expect(valid.firstMismatch.native.positions).toEqual({ anchor: 9, head: 1 });
    expect(valid.missingActions).toEqual(["caret:after-click", "caret:after-End"]);
    for (const action of actions) expect(valid.actions[action]).not.toBeNull();
    expect(valid.earlyBindingNotCaptured).toBe(true);
    expect(selected.caret).toBeNull();
  });

  const caretData = (): Data => {
    const value: Data = clone(data);
    value.caretBoundaries = [
      {
        stage: "after-click",
        ownerRecordStage: "caret:after-click",
        at: 5,
        native: {
          anchor: { inside: true, noneditableLeaf: false, position: 5 },
          head: { inside: true, noneditableLeaf: false, position: 5 },
          text: secret,
        },
        wide: true,
        rich: true,
        rawDom: secret,
      },
      {
        stage: "after-End",
        ownerRecordStage: "caret:after-End",
        at: 7,
        native: {
          anchor: { inside: false, noneditableLeaf: true, position: 9, nodeAttrs: secret },
          head: { inside: true, noneditableLeaf: false, position: 9 },
          text: secret,
        },
        wide: false,
        rich: false,
        focus: { activeLabel: secret },
        error: secret,
      },
    ];
    for (const [label, at, pos] of [
      ["caret:after-click", 5, 5],
      ["caret:after-End", 7, 9],
    ] as const) {
      const captured: Json & { stage: string } = {
        ...clone(frame),
        stage: label,
        at,
        pm: { anchor: pos, head: pos, empty: true, type: "text" },
      };
      (captured.native as Json).positions = { anchor: pos, head: pos };
      value.critical.push(captured);
      value.actionBoundaries.push(captured);
    }
    value.totals.critical += 2;
    value.totals.actionBoundaries += 2;
    return value;
  };

  test("caret boundaries: exact, private and nullable inputs", () => {
    const caretValid = run("caret_valid", { payload: caretData() }) as {
      missingActions: string[];
      actions: Record<string, Json & { native: Json; focus: Json }>;
    };
    expect(caretValid.missingActions).toEqual([]);
    const click = caretValid.actions["caret:after-click"];
    const end = caretValid.actions["caret:after-End"];
    expect(click?.caret).toEqual({
      anchor: { inside: true, noneditableLeaf: false, position: 5 },
      head: { inside: true, noneditableLeaf: false, position: 5 },
      wide: true,
      rich: true,
    });
    expect(end?.caret).toEqual({
      anchor: { inside: false, noneditableLeaf: true, position: 9 },
      head: { inside: true, noneditableLeaf: false, position: 9 },
      wide: false,
      rich: false,
    });
    expect(click?.owner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(end?.owner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(click?.pm).toEqual({ anchor: 5, head: 5, empty: true, type: "text" });
    expect(end?.native.positions).toEqual({ anchor: 9, head: 9 });
    expect(end?.focus.editor).toBe(true);

    const unknown = caretData();
    (unknown.caretBoundaries as Json[])[1] = {
      stage: "after-End",
      ownerRecordStage: "caret:after-End",
      at: 7,
      native: { anchor: { inside: null, noneditableLeaf: null, position: null }, head: {} },
    };
    const caretUnknown = run("caret_unknown", { payload: unknown }) as {
      actions: Record<string, Json>;
    };
    expect(caretUnknown.actions["caret:after-End"]?.caret).toEqual({
      anchor: { inside: null, noneditableLeaf: null, position: null },
      head: { inside: null, noneditableLeaf: null, position: null },
      wide: null,
      rich: null,
    });
  });

  test("caret boundaries: 15 typed, shape and overflow rejection controls", () => {
    const rejects: [string, Data, string][] = [];
    const base = caretData();
    const boundaries = base.caretBoundaries as Json[];
    for (const [label, value] of [
      ["array", secret],
      ["overflow", [...boundaries, ...boundaries]],
      ["duplicate", [boundaries[0], boundaries[0]]],
      ["entry", [secret]],
    ] as const) {
      const invalid = caretData();
      invalid.caretBoundaries = clone(value);
      rejects.push([label, invalid, "invalid_caret_boundary_schema"]);
    }
    for (const [label, key, value, reason] of [
      ["stage", "stage", secret, "invalid_caret_boundary_schema"],
      ["owner_stage", "ownerRecordStage", "caret:after-click", "invalid_caret_boundary_schema"],
      ["at", "at", secret, "invalid_caret_boundary_schema"],
      ["native", "native", secret, "invalid_caret_boundary_schema"],
      ["wide", "wide", "true", "invalid_boolean_schema"],
      ["rich", "rich", 1, "invalid_boolean_schema"],
    ] as const) {
      const invalid = caretData();
      ((invalid.caretBoundaries as Json[])[1] as Json)[key] = value;
      rejects.push([label, invalid, reason]);
    }
    for (const [label, key, value, reason] of [
      ["endpoint", null, secret, "invalid_caret_boundary_schema"],
      ["inside", "inside", 1, "invalid_boolean_schema"],
      ["leaf", "noneditableLeaf", "false", "invalid_boolean_schema"],
      ["position_type", "position", "9", "invalid_position_schema"],
      ["position_range", "position", 1_000_000_001, "invalid_position_schema"],
    ] as const) {
      const invalid = caretData();
      const native = ((invalid.caretBoundaries as Json[])[1] as { native: Json }).native;
      if (key === null) native.anchor = value;
      else (native.anchor as Json)[key] = value;
      rejects.push([label, invalid, reason]);
    }
    expect(rejects).toHaveLength(15);
    for (const [label, payload, reason] of rejects) {
      expect(run(`caret_reject_${label}`, { payload }), label).toEqual(reject(reason));
    }
  });

  test("hostile, partial and legacy inputs", () => {
    const hostile = clone(data);
    const hostileFrame = hostile.frames[0] as Json & { auth: Json; focus: Json; native: Json };
    hostileFrame.stage = "keydown:" + secret;
    hostileFrame.auth.scope = secret;
    hostileFrame.focus.activeTag = secret;
    hostileFrame.native.positions = { unknown: secret };
    hostileFrame.native.text = secret;
    const safe = (run("hostile", { payload: hostile }).tail as Json[])[0] as Json & {
      auth: Json;
      native: Json;
    };
    expect(safe.stage).toBe("unknown");
    expect(safe.auth.scope).toBe("unknown");
    expect(safe.native.equalsKnownFixtureCjkEmoji).toBe(false);
    expect(safe.native.mappingUnknown).toBe(true);
    expect(safe.at).toBe(10);

    const truncated = clone(data);
    truncated.totals.frames = 501;
    truncated.dropped.frames = 500;
    expect(
      (run("truncated", { payload: truncated }).counts as Record<string, Json>).frames?.dropped,
    ).toBe(500);
    const legacy: Partial<typeof data> = clone(data);
    delete legacy.totals;
    delete legacy.dropped;
    expect(
      (run("legacy_unknown", { payload: legacy }).counts as Record<string, Json>).frames?.unknown,
    ).toBe(true);
  });

  test("14 rejection controls", () => {
    const inconsistent = clone(data);
    inconsistent.totals.frames = 2;
    const malformedSnapshot = clone(data);
    (malformedSnapshot.frames[0] as Json).native = secret;
    const rejects: [string, Options, string][] = [
      ["path", { ref: { ...reference, file: "../" + secret } }, "invalid_attachment_reference"],
      [
        "content_type",
        { ref: { ...reference, contentType: "text/plain" } },
        "invalid_attachment_reference",
      ],
      [
        "missing",
        { ref: { ...reference, file: "attachments/" + "b".repeat(40) } },
        "missing_or_invalid_attachment_member",
      ],
      ["malformed", { payload: new TextEncoder().encode("{" + secret) }, "invalid_attachment_data"],
      [
        "oversize",
        { payload: new TextEncoder().encode(" ".repeat(1024 * 1024 + 1)) },
        "attachment_size_limit",
      ],
      ["schema", { payload: { frames: secret } }, "invalid_event_schema_or_limit"],
      ["duplicate", { duplicate: true }, "missing_or_invalid_attachment_member"],
      [
        "events",
        { test: Array.from({ length: 10001 }, () => "{}").join("\n") },
        "test_event_limit",
      ],
      ["bad_test", { test: "{" + secret }, "invalid_attachment_data"],
      [
        "non_json_number",
        { payload: new TextEncoder().encode(JSON.stringify(data).replace('"at":10', '"at":NaN')) },
        "invalid_attachment_data",
      ],
      ["no_reference", { test: "{}" }, "missing_or_duplicate_attachment"],
      [
        "member_limit",
        {
          extra: Array.from({ length: 4096 }, (_, index): [string, string] => [
            `unrelated/${String(index)}`,
            "",
          ]),
        },
        "member_limit",
      ],
      ["counts", { payload: inconsistent }, "inconsistent_collection_counts"],
      ["nested_schema", { payload: malformedSnapshot }, "invalid_snapshot_schema"],
    ];
    expect(rejects).toHaveLength(14);
    for (const [label, options, reason] of rejects) {
      expect(run(label, options), label).toEqual(reject(reason));
    }
  });

  test("the attachment record must be a plain, unencrypted file", () => {
    const unix = (mode: number) => ({ [member]: { mode, os: 3 } });
    expect(run("regular_mode", { modes: unix(0o100644) }).available).toBe(true);
    const dosRegular = { [member]: { mode: 0o100644, os: 0 } };
    expect(run("dos_regular_mode", { modes: dosRegular }).available).toBe(true);
    const rejects: [string, Options][] = [
      ["symlink", { modes: unix(0o120777) }],
      ["fifo", { modes: unix(0o010644) }],
      ["encrypted", { edit: (archive) => markEncrypted(archive, member) }],
      // Python reads the mode whatever system made the record.
      ["dos_symlink", { modes: { [member]: { mode: 0o120777, os: 0 } } }],
      // A later "../" alias of the name must not stand in for the record that is read.
      [
        "alias_symlink",
        {
          extra: [["../" + member, JSON.stringify(data)]],
          modes: { ...unix(0o120777), ["../" + member]: { mode: 0o100644, os: 3 } },
        },
      ],
    ];
    for (const [label, options] of rejects) {
      expect(run(label, options), label).toEqual(reject("missing_or_invalid_attachment_member"));
    }
  });

  test("a CRC-32 mismatch is invalid data, never a valid digest", () => {
    for (const target of [member, "test.trace"]) {
      expect(run(`bad_crc ${target}`, { edit: (archive) => corruptCrc(archive, target) })).toEqual(
        reject("invalid_attachment_data"),
      );
    }
    // Outside the digest a corrupt member fails the summary, as Python's read did.
    const work = mkdtempSync(join(tmpdir(), "fvoci-trace-summary."));
    try {
      const archive = join(work, "trace.zip");
      const network = JSON.stringify({ type: "console", messageType: "error", text: "x" });
      writeFileSync(archive, corruptCrc(zipOf([["0-trace.network", network]]), "0-trace.network"));
      const result = Bun.spawnSync([process.execPath, cli, archive], {
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(result.exitCode).toBe(1);
      expect(result.stdout.toString()).toBe("");
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  });

  test("Python float and integer tokens keep their types", () => {
    const floats = clone(data);
    (floats.frames[0] as Json).at = 10.5;
    const text = JSON.stringify(floats).replace('"bindingGeneration":1', '"bindingGeneration":1.0');
    const archive = zipOf([
      ["0-trace.network", JSON.stringify(network)],
      ["test.trace", JSON.stringify({ type: "after", attachments: [reference] })],
      [member, text],
    ]);
    const line =
      summarize(archive)
        .split("\n")
        .find((row) => row.startsWith("w3-template-diagnostic ")) ?? "";
    // A float `at` keeps Python's repr; a float where an int is required is unknown.
    expect(line).toContain(
      '"tail":[{"at":10.5,"stage":"ShiftHome:original-return","bindingGeneration":null,',
    );
  });
});
