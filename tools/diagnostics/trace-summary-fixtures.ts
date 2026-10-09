import type { Reason } from "./typeddiagnostic-schema";

// Independently declared from the original fixture, never generated from a public digest.
export const PRIVATE = "DIAGNOSTIC_PRIVATE_VALUE";
export const SENTINELS = [
  PRIVATE,
  "NATIVE_ID_PRIVATE",
  "private.example",
  "한글과",
  "😀",
  "rawFuture",
  "activeLabel",
  "pmDocument",
  "password",
];
export function fixture() {
  const frame = {
    at: 10,
    stage: "ShiftHome:original-return",
    owner: "[1,2,1,3,4,5,6]",
    bindingGeneration: 1,
    nativeId: "NATIVE_ID_PRIVATE",
    native: { inside: true, text: "한글과 😀 링크", positions: { anchor: 9, head: 1 } },
    pm: { anchor: 9, head: 1, empty: false, type: "text", marks: [{ href: PRIVATE }] },
    focus: {
      activeTag: "DIV",
      activeLabel: PRIVATE,
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
      password: PRIVATE,
    },
    updates: 0,
    localUpdates: 0,
    generationUpdates: 0,
    generationLocalUpdates: 0,
    bubble: { visibility: "visible", opacity: "1" },
    dialog: false,
    pmDocument: { type: "doc", attrs: { rawFuture: PRIVATE }, text: PRIVATE },
    unknown: `https://private.example/${PRIVATE}?password=${PRIVATE}`,
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
  const boundaries = actions.map((stage) => ({ ...structuredClone(frame), stage }));
  return {
    frames: [frame],
    critical: boundaries,
    actionBoundaries: boundaries.slice(0, -1),
    caretBoundaries: [] as ReturnType<typeof caretBoundary>[],
    ownerChanges: [
      {
        at: 20,
        stage: "provider:status",
        previousOwner: frame.owner,
        owner: "[11,12,11,13,14,15,16]",
        bindingGeneration: 2,
      },
    ],
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
}
function caretBoundary(
  stage: string,
  at: number,
  position: number,
  inside: boolean,
  leaf: boolean,
  wide: boolean,
) {
  return {
    stage,
    at,
    ownerRecordStage: `caret:${stage}`,
    native: {
      anchor: { inside, noneditableLeaf: leaf, position },
      head: { inside: true, noneditableLeaf: false, position },
      text: PRIVATE,
    },
    wide,
    rich: wide,
    rawDom: PRIVATE,
  };
}
export function caretFixture() {
  const data = fixture();
  data.caretBoundaries = [
    caretBoundary("after-click", 5, 5, true, false, true),
    caretBoundary("after-End", 7, 9, false, true, false),
  ];
  for (const boundary of data.caretBoundaries) {
    const position = boundary.native.anchor.position;
    const frame = {
      ...structuredClone(data.firstObservedState),
      at: boundary.at,
      stage: boundary.ownerRecordStage,
      pm: { ...data.firstObservedState.pm, anchor: position, head: position, empty: true },
      native: {
        ...data.firstObservedState.native,
        positions: { anchor: position, head: position },
      },
    };
    data.critical.push(frame);
    data.actionBoundaries.push(frame);
  }
  data.totals.critical += 2;
  data.totals.actionBoundaries += 2;
  return data;
}
type Fixture = ReturnType<typeof fixture>;
export const schemaCases: { name: string; input: unknown; reason?: Reason }[] = [];
function scenario(name: string, mutate: (data: Fixture) => void, reason?: Reason, caret = false) {
  const data = caret ? caretFixture() : fixture();
  mutate(data);
  schemaCases.push({ name, input: data, reason });
}
scenario("diagnostic positive fields", () => {});
scenario("caret positive projection", () => {}, undefined, true);
scenario(
  "caret nullable",
  (data) => {
    for (const boundary of data.caretBoundaries) {
      if (boundary.stage === "after-End")
        Object.assign(boundary, {
          native: { anchor: { inside: null, noneditableLeaf: null, position: null }, head: {} },
          wide: null,
          rich: null,
        });
    }
  },
  undefined,
  true,
);
scenario("hostile enum/shape", (data) => {
  const frame = data.firstObservedState;
  frame.stage = "keydown:" + PRIVATE;
  frame.auth.scope = PRIVATE;
  frame.focus.activeTag = PRIVATE;
  Object.assign(frame.native, { positions: { unknown: PRIVATE }, text: PRIVATE });
});
scenario("truncated counts", (data) => {
  data.totals.frames = 501;
  data.dropped.frames = 500;
});
scenario("legacy unknown counts", (data) => {
  Reflect.deleteProperty(data, "totals");
  Reflect.deleteProperty(data, "dropped");
});
for (const [name, value] of [
  ["array", PRIVATE],
  ["overflow", [...caretFixture().caretBoundaries, ...caretFixture().caretBoundaries]],
  ["duplicate", [caretFixture().caretBoundaries[0], caretFixture().caretBoundaries[0]]],
  ["entry", [PRIVATE]],
] as const)
  scenario(
    `caret reject ${name}`,
    (data) => {
      Object.assign(data, { caretBoundaries: value });
    },
    "invalid_caret_boundary_schema",
    true,
  );
for (const [name, key, value, reason] of [
  ["stage", "stage", PRIVATE, "invalid_caret_boundary_schema"],
  ["owner_stage", "ownerRecordStage", "caret:after-click", "invalid_caret_boundary_schema"],
  ["at", "at", PRIVATE, "invalid_caret_boundary_schema"],
  ["native", "native", PRIVATE, "invalid_caret_boundary_schema"],
  ["wide", "wide", "true", "invalid_boolean_schema"],
  ["rich", "rich", 1, "invalid_boolean_schema"],
] as const)
  scenario(
    `caret reject ${name}`,
    (data) => {
      for (const boundary of data.caretBoundaries)
        if (boundary.stage === "after-End") Object.assign(boundary, { [key]: value });
    },
    reason,
    true,
  );
for (const [name, key, value, reason] of [
  ["endpoint", "", PRIVATE, "invalid_caret_boundary_schema"],
  ["inside", "inside", 1, "invalid_boolean_schema"],
  ["leaf", "noneditableLeaf", "false", "invalid_boolean_schema"],
  ["position_type", "position", "9", "invalid_position_schema"],
  ["position_range", "position", 1_000_000_001, "invalid_position_schema"],
] as const)
  scenario(
    `caret reject ${name}`,
    (data) => {
      for (const boundary of data.caretBoundaries)
        if (boundary.stage === "after-End") {
          if (!key) Object.assign(boundary.native, { anchor: value });
          else Object.assign(boundary.native.anchor, { [key]: value });
        }
    },
    reason,
    true,
  );
scenario(
  "diagnostic reject schema",
  (data) => {
    Object.assign(data, { frames: PRIVATE });
  },
  "invalid_event_schema_or_limit",
);
scenario(
  "diagnostic reject counts",
  (data) => {
    data.totals.frames = 2;
  },
  "inconsistent_collection_counts",
);
scenario(
  "diagnostic reject nested_schema",
  (data) => {
    Object.assign(data.firstObservedState, { native: PRIVATE });
  },
  "invalid_snapshot_schema",
);

export const browserFixture = {
  source: "fvoci-playwright" as const,
  events: [
    {
      kind: "request" as const,
      at: 1,
      method: "GET",
      status: 404,
      resourceType: "document",
      url: `http://127.0.0.1:4000/s/${"S".repeat(43)}?code=QUERYSECRET`,
      duration: 12,
    },
    {
      kind: "request" as const,
      at: 1.5,
      method: "GET",
      status: 0,
      resourceType: "script",
      url: "http://127.0.0.1:4000/assets/index-DiwrgTda.js",
      duration: 12,
      failure: "net::ERR_ABORTED",
    },
    {
      kind: "request" as const,
      at: 1.7,
      method: "GET",
      status: 404,
      resourceType: "fetch",
      url: "http://127.0.0.1:4000/api/v1/invitations/PATHSECRET/accept",
      duration: 12,
    },
    {
      kind: "console" as const,
      at: 2,
      level: "error" as const,
      text: `fetch failed access_token=PARAMSECRET1 inviteToken=PARAMSECRET2 ${"B".repeat(45)} postgresql://u:DBSECRET@h/db`,
    },
    { kind: "pageerror" as const, at: 3, text: "a".repeat(200000) + " state=PARAMSECRET3" },
  ],
};
