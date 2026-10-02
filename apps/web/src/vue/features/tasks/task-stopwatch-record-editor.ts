import type { components } from "@/generated/api";
import { datetimeLocalInTimeZoneToIso, isoToDatetimeLocalInTimeZone } from "@/lib/datetime";

export type PersonalTimeRecord = components["schemas"]["TimeRecord"];
export type PersonalCorrectionBody = components["schemas"]["TimeCorrectionBody"];
export type PersonalManualBody = components["schemas"]["TimerManualBody"];

export type RecordEditor = {
  readonly record: Readonly<PersonalTimeRecord>;
  readonly timeZone: string;
  readonly initialStart: string;
  readonly initialEnd: string;
  readonly initialNote: string;
};

export type RecordDraft = {
  startedLocal: string;
  endedLocal: string;
  note: string;
  reason: string;
};

export type RecordIntent = {
  expectedActorId: string;
  expectedSessionId: string;
  requestId: string;
};

/** Minute inputs are an editing convenience, not a new precision contract.
 * Keep the raw server instant (including an overlap's selected offset and
 * subsecond precision) when the user leaves its displayed field unchanged. */
export function openRecordEditor(record: PersonalTimeRecord, timeZone: string): RecordEditor {
  return Object.freeze({
    record: Object.freeze({ ...record }),
    timeZone,
    initialStart: isoToDatetimeLocalInTimeZone(record.startedAt, timeZone),
    initialEnd: record.endedAt ? isoToDatetimeLocalInTimeZone(record.endedAt, timeZone) : "",
    initialNote: record.note ?? "",
  });
}

function validRange(start: string, end: string): boolean {
  // A correction may keep a zero-length segment or an interval shorter than a
  // millisecond. Date.parse loses microseconds, so range authority stays on the
  // server; the client only rejects obvious invalid/reversed millisecond values.
  return (
    Boolean(start && end) &&
    Number.isFinite(Date.parse(start)) &&
    Number.isFinite(Date.parse(end)) &&
    Date.parse(end) >= Date.parse(start)
  );
}

export function correctionFromDraft(
  editor: RecordEditor,
  draft: RecordDraft,
  intent: RecordIntent,
): Readonly<PersonalCorrectionBody> | undefined {
  const startedAt =
    draft.startedLocal === editor.initialStart
      ? editor.record.startedAt
      : datetimeLocalInTimeZoneToIso(draft.startedLocal, editor.timeZone);
  const endedAt =
    draft.endedLocal === editor.initialEnd && editor.record.endedAt !== null
      ? editor.record.endedAt
      : datetimeLocalInTimeZoneToIso(draft.endedLocal, editor.timeZone);
  if (!validRange(startedAt, endedAt) || !draft.reason.trim()) return;
  return Object.freeze({
    ...intent,
    kind: editor.record.kind,
    expectedRevision: editor.record.revision,
    expectedStartedAt: editor.record.startedAt,
    expectedEndedAt: editor.record.endedAt,
    expectedNote: editor.record.note,
    startedAt,
    endedAt,
    note: draft.note === editor.initialNote ? editor.record.note : draft.note,
    reason: draft.reason.trim(),
  });
}

export function manualFromDraft(
  draft: RecordDraft,
  timeZone: string,
  intent: RecordIntent,
): Readonly<PersonalManualBody> | undefined {
  const startedAt = datetimeLocalInTimeZoneToIso(draft.startedLocal, timeZone);
  const endedAt = datetimeLocalInTimeZoneToIso(draft.endedLocal, timeZone);
  if (
    !validRange(startedAt, endedAt) ||
    Date.parse(endedAt) <= Date.parse(startedAt) ||
    !draft.reason.trim()
  )
    return;
  return Object.freeze({
    ...intent,
    startedAt,
    endedAt,
    note: draft.note === "" ? null : draft.note,
    reason: draft.reason.trim(),
  });
}
