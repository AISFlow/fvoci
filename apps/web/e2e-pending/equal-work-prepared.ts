// Private observation DTO for an already admitted parent allocation. This
// decoder loads no credentials, grants no authority and starts no fixture.
import { strict as assert } from "node:assert";
import { z } from "zod";
import {
  workload,
  workloadHash,
} from "../../../tools/selected-backend-ci/drivers/equal-work-policy";
import type { HostedSeed } from "../../../tools/turso/ui-equal-work";
import { UUID_RE } from "./collab-wire";

const uuid = z
  .string()
  .regex(UUID_RE)
  .refine((value) => value === value.toLowerCase());
const sha = z.string().regex(/^[a-f0-9]{40}$/);
const hash = z.string().regex(/^[a-f0-9]{64}$/);
const schema = z
  .object({
    schema: z.literal(1),
    backend: z.literal("libsql-remote"),
    setupNeeded: z.literal(false),
    namespace: z.string().regex(/^tui-[a-f0-9]{20}$/),
    ownerEmail: z.string(),
    memberEmail: z.string(),
    workspaceSlug: z.string(),
    source: sha,
    tree: sha,
    schemaCurrent: z.literal(true),
    commit: z.literal("confirmed"),
    lifecycleDrain: z.literal("confirmed"),
    leases: z.literal(0),
    equalWork: z
      .object({
        schema: z.literal(1),
        flow: z.enum(["on", "off"]),
        workloadHash: z.literal(workloadHash),
        targetSha256: hash,
        ownerId: uuid,
        memberId: uuid,
        workspaceId: uuid,
        documentId: uuid,
        documentDisplayId: z
          .string()
          .min(1)
          .max(128)
          .regex(/^[A-Za-z0-9][A-Za-z0-9_-]*$/),
        initialBody: z.literal(""),
      })
      .strict(),
  })
  .loose();

export function equalWorkPreparedMode(
  flag: string | undefined,
  backend: string | undefined,
): boolean {
  assert.ok(flag === undefined || flag === "1", "equal-work prepared flag must be absent or 1");
  if (flag === undefined) return false;
  assert.equal(backend, "libsql-remote", "prepared equal work requires the admitted remote parent");
  return true;
}
export interface PreparedEqualWork {
  allocation: HostedSeed;
  displayId: string;
}
export function parsePreparedEqualWork(
  binding: unknown,
  expected: {
    backend: string;
    flow: string;
    namespace: string;
    source: string;
    tree: string;
    targetSha256: string;
    ownerEmail: string;
    memberEmail: string;
    workspaceSlug: string;
  },
): PreparedEqualWork {
  const result = schema.safeParse(binding);
  assert.ok(result.success, "equal-work prepared binding refused");
  const value = result.data,
    work = value.equalWork;
  assert.ok(
    value.backend === expected.backend &&
      work.flow === expected.flow &&
      value.namespace === expected.namespace &&
      value.source === expected.source &&
      value.tree === expected.tree &&
      work.targetSha256 === expected.targetSha256,
    "equal-work prepared source/target/flow binding refused",
  );
  assert.ok(
    value.ownerEmail === expected.ownerEmail &&
      value.memberEmail === expected.memberEmail &&
      value.workspaceSlug === expected.workspaceSlug &&
      value.workspaceSlug === value.namespace &&
      value.ownerEmail === value.namespace + "-owner@example.invalid" &&
      value.memberEmail === value.namespace + "-member@example.invalid",
    "equal-work prepared actors refused",
  );
  assert.notEqual(work.ownerId, work.memberId);
  return {
    displayId: work.documentDisplayId,
    allocation: {
      namespace: value.namespace,
      owner: work.ownerId,
      member: work.memberId,
      workspace: work.workspaceId,
      document: work.documentId,
      logical: {
        users: workload.users,
        documents: workload.documents,
        roles: ["owner", "member"],
        initialBody: work.initialBody,
        preparation: workload.preparation,
      },
    },
  };
}
