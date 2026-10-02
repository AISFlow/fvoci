import { describe, expect, test } from "bun:test";
import {
  correctionFromDraft,
  manualFromDraft,
  openRecordEditor,
  type PersonalTimeRecord,
} from "./task-stopwatch-record-editor";

const intent = { expectedActorId: "actor", expectedSessionId: "session", requestId: "request" };
const row: PersonalTimeRecord = {
  id: "record",
  kind: "segment",
  revision: 2,
  runId: "run",
  reservedLegacy: false,
  startedAt: "2026-01-01T23:59:59.123456Z",
  endedAt: "2026-01-01T23:59:59.623456Z",
  note: null,
};

describe("personal record editor exact server anchors", () => {
  test("note-only editing preserves raw microsecond CAS and the 500ms interval", () => {
    const editor = openRecordEditor(row, "Asia/Seoul");
    const body = correctionFromDraft(
      editor,
      {
        startedLocal: editor.initialStart,
        endedLocal: editor.initialEnd,
        note: "read chapter 2",
        reason: "  fix note  ",
      },
      intent,
    );
    expect(body).toEqual({
      ...intent,
      kind: "segment",
      expectedRevision: 2,
      expectedStartedAt: row.startedAt,
      expectedEndedAt: row.endedAt,
      expectedNote: null,
      startedAt: row.startedAt,
      endedAt: row.endedAt,
      note: "read chapter 2",
      reason: "fix note",
    });
    expect(Object.isFrozen(body)).toBe(true);
    expect(Object.isFrozen(editor.record)).toBe(true);
  });

  test("unchanged overlap field keeps the later instant; deliberate changes use the existing earlier-fold adapter", () => {
    const later = {
      ...row,
      startedAt: "2026-11-01T06:30:15.500Z",
      endedAt: "2026-11-01T07:30:00Z",
    };
    const editor = openRecordEditor(later, "America/New_York");
    expect(editor.initialStart).toBe("2026-11-01T01:30");
    const unchanged = correctionFromDraft(
      editor,
      {
        startedLocal: editor.initialStart,
        endedLocal: editor.initialEnd,
        note: "",
        reason: "review",
      },
      intent,
    );
    expect(unchanged?.startedAt).toBe(later.startedAt);
    expect(unchanged?.note).toBe(null);
    const edited = correctionFromDraft(
      editor,
      {
        startedLocal: "2026-11-01T01:31",
        endedLocal: editor.initialEnd,
        note: "",
        reason: "review",
      },
      intent,
    );
    expect(edited?.startedAt).toBe("2026-11-01T05:31:00.000Z");
    expect(edited?.expectedStartedAt).toBe(later.startedAt);
  });

  test("DST gap and missing reason reject submission without manufacturing an instant", () => {
    expect(
      manualFromDraft(
        {
          startedLocal: "2026-03-08T02:30",
          endedLocal: "2026-03-08T04:00",
          note: "",
          reason: "manual reading",
        },
        "America/New_York",
        intent,
      ),
    ).toBeUndefined();
    expect(
      manualFromDraft(
        {
          startedLocal: "2026-03-08T01:30",
          endedLocal: "2026-03-08T04:00",
          note: "",
          reason: "   ",
        },
        "America/New_York",
        intent,
      ),
    ).toBeUndefined();
    const body = manualFromDraft(
      {
        startedLocal: "2026-03-08T01:30",
        endedLocal: "2026-03-08T04:00",
        note: "",
        reason: "reading",
      },
      "America/New_York",
      intent,
    );
    expect(body?.startedAt).toBe("2026-03-08T06:30:00.000Z");
    expect(body?.endedAt).toBe("2026-03-08T08:00:00.000Z");
  });

  test("null and empty notes remain distinct; unresolved manual anchors require an explicit end", () => {
    const editor = openRecordEditor(
      { ...row, kind: "manual", endedAt: null, note: "", reservedLegacy: true },
      "Asia/Seoul",
    );
    const draft = {
      startedLocal: editor.initialStart,
      endedLocal: "",
      note: "",
      reason: "close historical entry",
    };
    expect(correctionFromDraft(editor, draft, intent)).toBeUndefined();
    const body = correctionFromDraft(editor, { ...draft, endedLocal: "2026-01-02T09:01" }, intent);
    expect(body?.expectedEndedAt).toBe(null);
    expect(body?.expectedNote).toBe("");
    expect(body?.note).toBe("");
    expect(body?.startedAt).toBe(row.startedAt);
  });
});
