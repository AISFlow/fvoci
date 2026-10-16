// Pure selected PRODUCT4 policy shared after admission and by browser adapters.
// No SDK initialization, process owner, environment or filesystem access.
import { createHash } from "node:crypto";
import { UUID_RE } from "../../apps/web/e2e-pending/collab-wire.ts";
import type { FinalBinding } from "../../apps/web/e2e-pending/selected-final-contract.ts";

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

const refuse = (): never => {
  throw new Error("SELECTED_FINAL_INPUT_REFUSED");
};

export function finalTitles(binding: Pick<FinalBinding, "suite" | "browser" | "flow">): string[] {
  return binding.suite === "products"
    ? [...PRODUCT_TITLES]
    : [boundaryTitle(binding.browser, binding.flow)];
}

export function assertFinalInventory(actual: string[], binding: FinalBinding): void {
  if (JSON.stringify(actual) !== JSON.stringify(finalTitles(binding))) refuse();
}

/** Exact public metadata values; Zod continues to own browser input parsing. */
export function selectedMetadata(sha256: string, binding: FinalBinding) {
  return {
    actorBindingSha256: sha256,
    source: binding.source,
    tree: binding.tree,
    selectedBackend: binding.backend,
    selectedFlow: binding.flow,
    browser: binding.browser,
    suite: binding.suite,
    titles: finalTitles(binding),
  };
}

export interface SelectedSyncWrite {
  workspaceId: string;
  documentId: string;
  actorId: string;
  sha256: string;
  length: number;
  empty: boolean;
  applied: boolean;
}

export interface SelectedNativeReceipt {
  seq: number;
  op: string;
  actor: string;
  sha: string;
  len: number;
}

/** The parent passes only new receipts for this exact target after proving
 * original rows retained. Every nonempty successful wire write is consumed
 * once; an additional denied/foreign/duplicate native operation fails closed. */
export function assertSelectedSuccessfulWrites(
  receipts: SelectedNativeReceipt[],
  writes: SelectedSyncWrite[],
  workspaceId: string,
  documentId: string,
): void {
  const successful = writes.filter(
    (w) => w.workspaceId === workspaceId && w.documentId === documentId,
  );
  if (
    successful.some(
      (w) =>
        !w.applied ||
        !UUID_RE.test(w.actorId) ||
        !/^[a-f0-9]{64}$/.test(w.sha256) ||
        !Number.isSafeInteger(w.length) ||
        w.length < 1 ||
        (w.empty &&
          (w.length !== 2 ||
            w.sha256 !==
              createHash("sha256")
                .update(new Uint8Array([0, 0]))
                .digest("hex"))),
    ) ||
    new Set(receipts.map((r) => r.op)).size !== receipts.length ||
    new Set(receipts.map((r) => r.seq)).size !== receipts.length
  )
    throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
  const remaining = successful.filter((w) => !w.empty);
  for (const receipt of receipts) {
    if (!Number.isSafeInteger(receipt.seq) || receipt.seq < 1 || !UUID_RE.test(receipt.op))
      throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
    const index = remaining.findIndex(
      (w) => w.actorId === receipt.actor && w.sha256 === receipt.sha && w.length === receipt.len,
    );
    if (index < 0) throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
    remaining.splice(index, 1);
  }
  if (remaining.length) throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
}

export function assertSelectedTargetWrites(
  targets: { workspaceId: string; documentId: string; receipts: SelectedNativeReceipt[] }[],
  writes: SelectedSyncWrite[],
) {
  const keys = targets.map((t) => `${t.workspaceId}:document:${t.documentId}`);
  if (
    new Set(keys).size !== keys.length ||
    writes.some((w) => !keys.includes(`${w.workspaceId}:document:${w.documentId}`))
  )
    throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
  for (const target of targets)
    assertSelectedSuccessfulWrites(target.receipts, writes, target.workspaceId, target.documentId);
}
