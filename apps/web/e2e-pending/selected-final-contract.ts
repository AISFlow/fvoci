// Inputs and discovery for an already allocated normal server. This is not
// runtime admission: the caller owns source/browser/native/target sealing.
import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";
import { z } from "zod";

export const FINAL_SPEC = "selected-final-acceptance.ts";
export const PRODUCT_TITLES = [
  "selected final ON: personal capture, matching ACK and private native readback",
  "selected final ON: private project, references, attachment and grant coedit",
  "selected final ON: role change and removal refuse live writes without effects",
  "selected final ON: reconnect unsent edits, personal undo and structured history",
] as const;
export const browsers = ["chromium", "firefox", "webkit"] as const;
export const flows = ["on", "off"] as const;
export type FinalBrowser = (typeof browsers)[number];
export type FinalFlow = (typeof flows)[number];
export const boundaryTitle = (browser: FinalBrowser, flow: FinalFlow) =>
  `selected final ${browser} ${flow}: Ctrl Home End before keyup and focus repair`;

const actor = z
  .object({
    userId: z.uuid(),
    email: z.email(),
    password: z.string().min(1).max(1024),
  })
  .strict();
const bindingSchema = z
  .object({
    schema: z.literal(1),
    backend: z.enum(["postgres", "sqlite", "libsql-remote"]),
    source: z.string().regex(/^[a-f0-9]{40}$/),
    tree: z.string().regex(/^[a-f0-9]{40}$/),
    browser: z.enum(browsers),
    flow: z.enum(flows),
    suite: z.enum(["products", "boundary"]),
    namespace: z.string().regex(/^tui-[a-f0-9]{20}$/),
    workspaceId: z.uuid(),
    workspaceSlug: z.string(),
    setupNeeded: z.literal(false),
    schemaCurrent: z.literal(true),
    commit: z.literal("confirmed"),
    lifecycleDrain: z.literal("confirmed"),
    leases: z.literal(0),
    owner: actor,
    member: actor,
    outsider: actor,
  })
  .strict();
export type FinalBinding = z.infer<typeof bindingSchema>;
type Env = Record<string, string | undefined>;
const refuse = (): never => {
  throw new Error("SELECTED_FINAL_INPUT_REFUSED");
};

export function validateFinalBinding(value: unknown, env: Env): FinalBinding {
  const result = bindingSchema.safeParse(value);
  if (!result.success) return refuse();
  const b = result.data;
  const expected = [
    [b.backend, env.FVOCI_E2E_SELECTED_BACKEND],
    [b.flow, env.FVOCI_E2E_SELECTED_FLOW],
    [b.source, env.FVOCI_E2E_SELECTED_SOURCE],
    [b.tree, env.FVOCI_E2E_SELECTED_TREE],
    [b.browser, env.FVOCI_E2E_FINAL_BROWSER],
    [b.suite, env.FVOCI_E2E_FINAL_SUITE],
    [b.namespace, env.FVOCI_E2E_TURSO_NAMESPACE],
  ];
  if (
    expected.some(([a, c]) => a !== c) ||
    env.FVOCI_E2E_PENDING !== undefined ||
    b.workspaceSlug !== b.namespace ||
    (b.suite === "products" && (b.flow !== "on" || b.browser !== "chromium")) ||
    new Set([b.owner.userId, b.member.userId, b.outsider.userId]).size !== 3 ||
    new Set([b.owner.email, b.member.email, b.outsider.email].map((s) => s.toLowerCase())).size !==
      3 ||
    b.owner.email !== `${b.namespace}-owner@example.invalid` ||
    b.member.email !== `${b.namespace}-member@example.invalid` ||
    b.outsider.email !== `${b.namespace}-outsider@example.invalid`
  )
    return refuse();
  return b;
}

export function finalTitles(binding: Pick<FinalBinding, "suite" | "browser" | "flow">): string[] {
  return binding.suite === "products"
    ? [...PRODUCT_TITLES]
    : [boundaryTitle(binding.browser, binding.flow)];
}

export function assertFinalInventory(actual: string[], binding: FinalBinding): void {
  if (JSON.stringify(actual) !== JSON.stringify(finalTitles(binding))) refuse();
}

export function assertFinalMetadata(
  metadata: unknown,
  sha256: string,
  binding: FinalBinding,
): void {
  const result = z
    .object({
      actorBindingSha256: z.literal(sha256),
      source: z.literal(binding.source),
      tree: z.literal(binding.tree),
      selectedBackend: z.literal(binding.backend),
      selectedFlow: z.literal(binding.flow),
      browser: z.literal(binding.browser),
      suite: z.literal(binding.suite),
      titles: z.array(z.string()),
    })
    .safeParse(metadata);
  if (!result.success) refuse();
  assertFinalInventory(result.data.titles, binding);
}

export function readFinalBinding(env: Env = process.env) {
  try {
    const path = env.FVOCI_E2E_FINAL_ACTOR_BINDING;
    if (!path || !isAbsolute(path)) return refuse();
    const stat = lstatSync(path);
    if (
      !stat.isFile() ||
      stat.isSymbolicLink() ||
      stat.nlink !== 1 ||
      stat.uid !== process.getuid?.() ||
      (stat.mode & 0o777) !== 0o600 ||
      stat.size > 16384
    )
      return refuse();
    const bytes = readFileSync(path);
    return {
      binding: validateFinalBinding(JSON.parse(bytes.toString("utf8")), env),
      sha256: createHash("sha256").update(bytes).digest("hex"),
    };
  } catch {
    return refuse();
  }
}
