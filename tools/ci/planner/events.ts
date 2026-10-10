import { diffPathsForPr, ensureCommitShas, validateSha, type Git } from "./git.ts";
import { pySorted, isMapping, type PyValue } from "./pyjson.ts";
import { OPT_IN_JOBS, type Workflow } from "./registry.ts";

// Event payload reading and the git-verified path set a pull request may
// narrow on. Only SHAs from the trusted event are used; never a path list.

export const KNOWN_EVENTS: ReadonlySet<string> = new Set([
  "pull_request",
  "push",
  "merge_group",
  "workflow_dispatch",
]);
export const ALWAYS_FULL_EVENTS: ReadonlySet<string> = new Set([
  "push",
  "merge_group",
  "workflow_dispatch",
]);

/** A payload shape the event cannot have; the plan refuses rather than guess. */
export class EventShapeError extends Error {}

const shaOrNull = (value: PyValue | undefined): string | null =>
  typeof value === "string" && validateSha(value) ? value : null;

/**
 * merge_group.base_sha and merge_group.head_sha, nothing else. A missing
 * group, a non-object or a value that is not 40 lowercase hex characters is
 * absent; merge_group selects the full matrix either way.
 */
export function mergeGroupShas(event: PyValue): [string | null, string | null] {
  if (!isMapping(event)) return [null, null];
  const group = event.get("merge_group");
  if (!isMapping(group)) return [null, null];
  return [shaOrNull(group.get("base_sha")), shaOrNull(group.get("head_sha"))];
}

function field(container: PyValue | undefined, key: string, what: string): PyValue {
  if (!isMapping(container)) throw new EventShapeError(`${what} is not an object`);
  return container.get(key) ?? null;
}

/**
 * The event's base and head values. A pull request's are returned as given
 * for the caller to validate; push keeps only well-formed SHAs, as
 * merge_group does. An event, pull_request, base or head that is present but
 * not an object is refused.
 */
export function eventShas(event: PyValue, eventName: string): [PyValue, PyValue] {
  if (eventName === "pull_request") {
    const pr = field(event, "pull_request", "event");
    if (pr === null) return [null, null];
    const sha = (side: string): PyValue => {
      const ref = field(pr, side, "pull_request");
      return ref === null ? null : field(ref, "sha", `pull_request.${side}`);
    };
    return [sha("base"), sha("head")];
  }
  if (eventName === "merge_group") return mergeGroupShas(event);
  if (eventName === "push") {
    return [shaOrNull(field(event, "before", "event")), shaOrNull(field(event, "after", "event"))];
  }
  return [null, null];
}

/**
 * Boolean opt-in inputs chosen by a workflow_dispatch event, fail closed.
 * Every other event selects nothing whatever its payload carries. GitHub
 * writes dispatch booleans as the strings "true"/"false"; a missing input is
 * not chosen, while an unknown input or any other value is an error.
 */
export function dispatchOptIns(
  workflow: Workflow,
  eventName: string,
  event: PyValue,
): { chosen: ReadonlySet<string>; error: string | null } {
  const none = new Set<string>();
  if (eventName !== "workflow_dispatch") return { chosen: none, error: null };
  if (!isMapping(event)) return { chosen: none, error: "DISPATCH_EVENT_INVALID" };
  const raw = event.get("inputs") ?? null;
  if (raw === null) return { chosen: none, error: null };
  if (!isMapping(raw)) return { chosen: none, error: "DISPATCH_INPUTS_INVALID" };
  const allowed = new Set(Object.values(OPT_IN_JOBS[workflow] ?? {}));
  if ([...raw.keys()].some((name) => !allowed.has(name))) {
    return { chosen: none, error: "DISPATCH_INPUTS_UNKNOWN" };
  }
  const chosen = new Set<string>();
  for (const [name, value] of raw) {
    if (value === true || value === "true") chosen.add(name);
    else if (!(value === false || value === "false")) {
      return { chosen: none, error: "DISPATCH_INPUT_VALUE_INVALID" };
    }
  }
  return { chosen, error: null };
}

export type ResolvedInputs = {
  paths: string[] | null;
  fatalError: string | null;
  forceFullReason: string | null;
  baseSha: string | null;
  headSha: string | null;
  mergeBaseSha: string | null;
  testedSha: string | null;
};

/**
 * Bind the tested merge to the exact PR head and a trusted base lineage.
 * GitHub can regenerate refs/pull/N/merge after the event base advanced. Only
 * a descendant of the event's trusted base may replace the first parent. The
 * caller still classifies both the cumulative PR and actual merge diffs.
 */
export function prCheckoutNarrowBlock(
  git: Git,
  testedSha: string,
  baseSha: string,
  headSha: string,
): string | null {
  const parents = git.commitParents(testedSha);
  if (parents.error !== null) return parents.error;
  if (parents.value.length !== 2) return "FULL_PR_CHECKOUT_NOT_MERGE";
  if (parents.value[1] !== headSha) return "FULL_PR_MERGE_PARENTS_MISMATCH";
  if (parents.value[0] !== baseSha && !git.isAncestor(baseSha, parents.value[0] as string)) {
    return "FULL_PR_MERGE_PARENTS_MISMATCH";
  }
  return null;
}

/** `tested` is GITHUB_SHA as the runner exported it (whitespace-trimmed). */
export function resolveSelectionInputs(
  git: Git,
  event: PyValue,
  eventName: string,
  tested: string,
): ResolvedInputs {
  const make = (over: Partial<ResolvedInputs>): ResolvedInputs => ({
    paths: null,
    fatalError: null,
    forceFullReason: null,
    baseSha: null,
    headSha: null,
    mergeBaseSha: null,
    testedSha: tested,
    ...over,
  });
  if (!validateSha(tested)) return make({ fatalError: "TESTED_SHA_INVALID", testedSha: null });
  const headNow = git.revParse("HEAD", false);
  if (headNow.error !== null) return make({ fatalError: "HEAD_REV_PARSE_FAILED" });
  if (headNow.value !== tested) return make({ fatalError: "TESTED_SHA_MISMATCH" });

  if (eventName === "workflow_dispatch" || !KNOWN_EVENTS.has(eventName)) return make({});
  if (eventName === "merge_group") {
    const [baseSha, headSha] = mergeGroupShas(event);
    return make({ baseSha, headSha });
  }
  if (eventName === "push") {
    const before = field(event, "before", "event");
    const after = field(event, "after", "event");
    return make({ baseSha: shaOrNull(before), headSha: shaOrNull(after) });
  }

  const [rawBase, rawHead] = eventShas(event, eventName);
  // The plan records strings as the event carried them and nothing else.
  const recorded = {
    baseSha: typeof rawBase === "string" ? rawBase : null,
    headSha: typeof rawHead === "string" ? rawHead : null,
  };
  const absent = (sha: PyValue) => sha === null || sha === "";
  if (absent(rawBase) || absent(rawHead)) {
    return make({ fatalError: "MISSING_BASE_OR_HEAD", ...recorded });
  }
  const { baseSha, headSha } = recorded;
  if (baseSha === null || headSha === null || !validateSha(baseSha) || !validateSha(headSha)) {
    return make({ fatalError: "SHA_INVALID", ...recorded });
  }
  const shas = { baseSha, headSha };

  const fetchError = ensureCommitShas(git, baseSha, headSha);
  if (fetchError) return make({ fatalError: fetchError, ...shas });

  const block = prCheckoutNarrowBlock(git, tested, baseSha, headSha);
  if (block) return make({ forceFullReason: block, ...shas });

  const pr = diffPathsForPr(git, baseSha, headSha);
  if (pr.error !== null) return make({ fatalError: pr.error, ...shas, mergeBaseSha: pr.mergeBase });
  const mergeBaseSha = pr.mergeBase;

  // A merge can contain conflict resolutions or injected files absent from
  // the PR head. Inspect the exact tested tree against its trusted first
  // parent even when both parents equal the event.
  const parents = git.commitParents(tested);
  if (parents.error !== null) return make({ fatalError: parents.error, ...shas, mergeBaseSha });
  const merged = git.diffPaths(parents.value[0] as string, tested);
  if (merged.error !== null) return make({ fatalError: merged.error, ...shas, mergeBaseSha });
  const paths = pySorted(new Set([...(pr.paths ?? []), ...merged.value]));
  return make({ paths, ...shas, mergeBaseSha });
}
