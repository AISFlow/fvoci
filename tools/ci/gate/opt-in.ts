// Manual opt-in jobs, re-derived from this run's event file.

import { KNOWN_EVENTS, OPT_IN_JOBS, isRecord, type Plan, type Workflow } from "./schema.ts";

export type OptIns = { ok: true; chosen: ReadonlySet<string> } | { ok: false; error: string };

/**
 * Boolean inputs chosen by a workflow_dispatch event, fail closed. Every other
 * event chooses nothing, whatever its payload carries. GitHub writes dispatch
 * booleans as the strings "true"/"false"; a missing input is not chosen, while
 * an unknown input or any other value is an error.
 */
export function dispatchOptIns(workflow: Workflow, eventName: string, event: unknown): OptIns {
  if (eventName !== "workflow_dispatch") return { ok: true, chosen: new Set() };
  if (!isRecord(event)) return { ok: false, error: "DISPATCH_EVENT_INVALID" };
  const raw = event.inputs;
  if (raw === undefined || raw === null) return { ok: true, chosen: new Set() };
  if (!isRecord(raw)) return { ok: false, error: "DISPATCH_INPUTS_INVALID" };
  const allowed = new Set(Object.values(OPT_IN_JOBS[workflow] ?? {}));
  if (Object.keys(raw).some((name) => !allowed.has(name))) {
    return { ok: false, error: "DISPATCH_INPUTS_UNKNOWN" };
  }
  const chosen = new Set<string>();
  for (const [name, value] of Object.entries(raw)) {
    if (value === true || value === "true") chosen.add(name);
    else if (!(value === false || value === "false")) {
      return { ok: false, error: "DISPATCH_INPUT_VALUE_INVALID" };
    }
  }
  return { ok: true, chosen };
}

/** How the gate reads this run's event file. */
export type EventFile =
  { kind: "missing-path" } | { kind: "malformed" } | { kind: "parsed"; value: unknown };

/**
 * The plan cannot override a manual opt-in: each opt-in job must be selected
 * exactly when this run's event chose its input. The event file is read only
 * for workflows that have opt-in jobs.
 */
export function gateOptInError(
  workflow: Workflow,
  plan: Plan,
  eventName: string,
  readEvent: () => EventFile,
): string | null {
  const optIns = OPT_IN_JOBS[workflow];
  if (!optIns || Object.keys(optIns).length === 0) return null;
  if (!KNOWN_EVENTS.has(eventName)) return "EVENT_NAME";
  const event = readEvent();
  if (event.kind === "missing-path") return "EVENT_PATH_MISSING";
  if (event.kind === "malformed") return "EVENT_MALFORMED";
  const chosen = dispatchOptIns(workflow, eventName, event.value);
  if (!chosen.ok) return chosen.error;
  for (const [job, input] of Object.entries(optIns)) {
    if (plan.jobs[job]?.selected !== chosen.chosen.has(input)) return `OPT_IN_MISMATCH ${job}`;
  }
  return null;
}
