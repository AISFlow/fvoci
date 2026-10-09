import { z } from "zod";

export const DIAGNOSTIC_NAME = "w3-template-native-selection-observation.json";
export const BROWSER_DIAGNOSTIC_NAME = "fvoci-browser-diagnostic.json";
export const ACTIONS = [
  "openDoc:original",
  "caret:after-click",
  "caret:after-End",
  "ShiftHome:original-return",
  "bubble:original-visible",
  "popup:original-open-focus",
  "compositionEnter:original-no-link",
  "Cancel:original-native-text",
  "Apply:original-selected-text",
  "save:original",
  "finally",
] as const;
export const REASONS = [
  "invalid_schema",
  "invalid_event_schema_or_limit",
  "invalid_boundary_schema",
  "invalid_caret_boundary_schema",
  "invalid_boolean_schema",
  "invalid_position_schema",
  "invalid_first_state_schema",
  "invalid_event_schema",
  "invalid_owner_schema",
  "invalid_snapshot_schema",
  "invalid_native_text_schema",
  "invalid_update_counts",
  "invalid_collection_counts",
  "inconsistent_collection_counts",
  "output_size_limit",
  "missing_or_duplicate_attachment",
  "invalid_attachment_reference",
  "missing_or_invalid_attachment_member",
  "attachment_size_limit",
  "invalid_attachment_data",
  "invalid_report",
  "foreign_report",
  "report_event_limit",
  "attachment_limit",
] as const;
export type Reason = (typeof REASONS)[number];
export const reasonSchema = z.enum(REASONS);

export function integerSchema(reason: Reason = "invalid_position_schema") {
  return z.number({ error: reason }).int(reason).min(0, reason).max(1_000_000_000, reason);
}
export function timeSchema(reason: Reason = "invalid_event_schema") {
  return z.number({ error: reason }).min(0, reason).max(1e12, reason);
}
const bool = z.boolean({ error: "invalid_boolean_schema" }).nullish();
const position = integerSchema().optional();
const positions = z.object(
  { anchor: position, head: position, unknown: z.unknown().optional() },
  { error: "invalid_position_schema" },
);
const ownerIds = z.tuple([
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
  integerSchema("invalid_owner_schema").nullable(),
]);
const owner = z
  .string({ error: "invalid_owner_schema" })
  .max(256, "invalid_owner_schema")
  .transform((value, ctx): unknown => {
    try {
      return JSON.parse(value) as unknown;
    } catch {
      ctx.addIssue({ code: "custom", message: "invalid_owner_schema" });
      return z.NEVER;
    }
  })
  .pipe(ownerIds)
  .optional();
const snapshotOptions = { error: "invalid_snapshot_schema" };
const native = z
  .object(
    {
      inside: bool,
      text: z
        .string({ error: "invalid_native_text_schema" })
        .refine((value) => Array.from(value).length <= 4096, "invalid_native_text_schema")
        .nullish(),
      positions: positions.nullish(),
    },
    snapshotOptions,
  )
  .optional();
const pm = z
  .object(
    { anchor: position, head: position, empty: bool, type: z.unknown().optional() },
    snapshotOptions,
  )
  .optional();
const focus = z
  .object(
    {
      activeTag: z.unknown().optional(),
      editor: bool,
      editorEditable: bool,
      composing: bool,
      domEditable: z.unknown().optional(),
    },
    snapshotOptions,
  )
  .optional();
const auth = z
  .object(
    {
      authenticated: bool,
      synced: bool,
      scope: z.unknown().optional(),
      status: z.unknown().optional(),
    },
    snapshotOptions,
  )
  .optional();
export const eventSchema = z.object(
  {
    at: timeSchema(),
    stage: z
      .string({
        error: "invalid_event_schema",
      })
      .max(512, "invalid_event_schema"),
    owner,
    previousOwner: owner,
    native,
    pm,
    focus,
    auth,
    nativePositions: positions.nullish(),
    nativeId: z.unknown().optional(),
    unknown: z.unknown().optional(),
    unavailable: z.unknown().optional(),
    bindingGeneration: z.unknown().optional(),
    eventBindingGeneration: z.unknown().optional(),
    retiredEvent: z.unknown().optional(),
    updates: z.unknown().optional(),
    localUpdates: z.unknown().optional(),
    generationUpdates: z.unknown().optional(),
    generationLocalUpdates: z.unknown().optional(),
    retiredUpdate: z.unknown().optional(),
    bubble: z.unknown().optional(),
    dialog: z.unknown().optional(),
  },
  { error: "invalid_event_schema_or_limit" },
);
const endpoint = z.object(
  { inside: bool, noneditableLeaf: bool, position: integerSchema().nullish() },
  {
    error: "invalid_caret_boundary_schema",
  },
);
const caret = z
  .object({
    stage: z.enum(["after-click", "after-End"], {
      error: "invalid_caret_boundary_schema",
    }),
    ownerRecordStage: z.string({
      error: "invalid_caret_boundary_schema",
    }),
    at: timeSchema("invalid_caret_boundary_schema"),
    native: z.object(
      { anchor: endpoint, head: endpoint },
      {
        error: "invalid_caret_boundary_schema",
      },
    ),
    wide: bool,
    rich: bool,
  })
  .refine(
    (value) => value.ownerRecordStage === `caret:${value.stage}`,
    "invalid_caret_boundary_schema",
  );
function events(limit: number, reason: Reason) {
  return z.array(eventSchema, { error: reason }).max(limit, reason);
}
export const observationSchema = z
  .object(
    {
      frames: events(512, "invalid_event_schema_or_limit"),
      critical: events(2048, "invalid_event_schema_or_limit"),
      ownerChanges: events(2048, "invalid_event_schema_or_limit"),
      observedMismatches: events(4096, "invalid_event_schema_or_limit"),
      actionBoundaries: events(16, "invalid_boundary_schema").default([]),
      caretBoundaries: z
        .array(caret, { error: "invalid_caret_boundary_schema" })
        .max(2, "invalid_caret_boundary_schema")
        .default([])
        .refine(
          (value) => new Set(value.map((item) => item.stage)).size === value.length,
          "invalid_caret_boundary_schema",
        ),
      firstObservedState: eventSchema.nullish(),
      firstRetiredEvent: eventSchema.nullish(),
      updates: integerSchema("invalid_update_counts"),
      localUpdates: integerSchema("invalid_update_counts"),
      bindingGeneration: z.unknown().optional(),
      totals: z.record(z.string(), z.unknown(), { error: "invalid_collection_counts" }).default({}),
      dropped: z
        .record(z.string(), z.unknown(), { error: "invalid_collection_counts" })
        .default({}),
    },
    { error: "invalid_schema" },
  )
  .refine((value) => value.localUpdates <= value.updates, "invalid_update_counts");
export type ObservationEvent = z.infer<typeof eventSchema>;

// These are private fixture inputs. Only the explicit public projection is emitted.
export const browserEventSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("request"),
    at: timeSchema(),
    method: z.unknown().optional(),
    status: z.unknown().optional(),
    resourceType: z.unknown().optional(),
    url: z.string().max(16384),
    duration: timeSchema(),
    failure: z.unknown().optional(),
  }),
  z.object({
    kind: z.literal("console"),
    at: timeSchema(),
    level: z.enum(["error", "warning"]),
    text: z.unknown().optional(),
  }),
  z.object({ kind: z.literal("pageerror"), at: timeSchema() }),
  z.object({ kind: z.literal("crash"), at: timeSchema() }),
]);
export const browserReportSchema = z.object({
  source: z.literal("fvoci-playwright"),
  events: z.array(browserEventSchema).max(10000),
});
