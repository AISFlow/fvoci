import {
  deepEqual,
  get,
  has,
  isMapping,
  pyReprList,
  type Mapping,
  type VerifyContext,
} from "./load.ts";

export type WriteScopes = Readonly<Record<string, readonly string[]>>;

// Jobs that may hold a write token in the tag-driven release workflow.
export const RELEASE_WRITE_SCOPES: WriteScopes = {
  build: ["packages"],
  index: ["packages"],
  publish: ["packages"],
  release: ["contents"],
};

// Image publication is separate from tag-driven product releases.
export const CI_BASE_WRITE_SCOPES: WriteScopes = {
  build: [],
  push: ["packages"],
  "push-manifest": ["packages"],
};

const READ_ONLY = { contents: "read" } as const;

/** Top-level token is read-only and each job writes only its listed scopes. */
export function verifyWorkflowWriteScopes(
  data: Mapping,
  name: string,
  allowed: WriteScopes,
): string[] {
  const errors: string[] = [];
  if (!deepEqual(get(data, "permissions"), READ_ONLY)) {
    errors.push(`${name}: top-level permissions must be exactly contents: read`);
  }
  const jobs = get(data, "jobs");
  if (!isMapping(jobs) || Object.keys(jobs).length === 0) {
    return [...errors, `${name}: jobs mapping missing`];
  }
  for (const [jobId, spec] of Object.entries(jobs)) {
    if (!isMapping(spec)) {
      errors.push(`${name}: ${jobId} must be a mapping`);
      continue;
    }
    const permissions = has(spec, "permissions") ? get(spec, "permissions") : {};
    if (!isMapping(permissions)) {
      errors.push(`${name}: ${jobId} permissions must be a scope mapping`);
      continue;
    }
    const writes = Object.entries(permissions)
      .filter(([, level]) => level === "write")
      .map(([scope]) => scope);
    const permitted = new Set(Object.hasOwn(allowed, jobId) ? allowed[jobId] : []);
    const extra = writes.filter((scope) => !permitted.has(scope)).sort();
    if (extra.length > 0) errors.push(`${name}: ${jobId} may not write ${pyReprList(extra)}`);
  }
  return errors;
}

type PermissionPin = {
  top: Mapping;
  /** Jobs that declare permissions; every other job must not declare any. */
  jobs: Readonly<Record<string, Mapping>>;
};

// The token permissions of every registered workflow, exactly as reviewed.
// Changing one is a reviewed edit of this table.
export const PERMISSION_PINS: Readonly<Record<string, PermissionPin>> = {
  "rust.yml": { top: READ_ONLY, jobs: {} },
  "web.yml": { top: READ_ONLY, jobs: {} },
  "install.yml": { top: READ_ONLY, jobs: {} },
  "documents.yml": { top: READ_ONLY, jobs: {} },
  "collab-engine.yml": { top: READ_ONLY, jobs: {} },
  "turso-test.yml": { top: READ_ONLY, jobs: {} },
  "ci-base-image.yml": {
    top: READ_ONLY,
    jobs: {
      push: { contents: "read", packages: "write" },
      "push-manifest": { contents: "read", packages: "write" },
    },
  },
  "release.yml": {
    top: READ_ONLY,
    jobs: {
      verify: { contents: "read", checks: "read", packages: "read" },
      build: { contents: "read", packages: "write" },
      index: { contents: "read", packages: "write" },
      dist: READ_ONLY,
      smoke: READ_ONLY,
      publish: { contents: "read", packages: "write" },
      release: { contents: "write" },
    },
  },
};

export function verifyPermissionPins(ctx: VerifyContext): string[] {
  const errors: string[] = [];
  for (const [file, pin] of Object.entries(PERMISSION_PINS)) {
    const data = ctx.workflows[file];
    if (!data) continue; // Missing and unparsable files are reported by the registry.
    if (!deepEqual(get(data, "permissions"), pin.top)) {
      errors.push(`${file}: top-level permissions changed from the pinned value`);
    }
    const jobs = get(data, "jobs");
    if (!isMapping(jobs)) continue;
    for (const [jobId, spec] of Object.entries(jobs)) {
      const expected = Object.hasOwn(pin.jobs, jobId) ? pin.jobs[jobId] : undefined;
      if (
        expected === undefined
          ? has(spec, "permissions")
          : !deepEqual(get(spec, "permissions"), expected)
      ) {
        errors.push(`${file}: ${jobId} permissions changed from the pinned value`);
      }
    }
    for (const jobId of Object.keys(pin.jobs)) {
      if (!has(jobs, jobId)) errors.push(`${file}: pinned permissions job ${jobId} missing`);
    }
  }
  return errors;
}
