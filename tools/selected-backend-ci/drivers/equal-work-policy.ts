import { createHash } from "node:crypto";

// Shared literal policy, imported by the browser and its allocated collector.
export const equalSpec = "e2e-pending/v060-equal-work.spec.ts";
export const equalTitle = "equal work: UI edit, matching durable ACK and fresh readback";
export const workload = {
  schema: 1,
  users: 2,
  documents: 1,
  warm: "equal work warm",
  edits: ["equal work one", "equal work two", "equal work three"],
  preparation: "fresh normal migrate --start; Vue setup; owner and member; one empty wiki",
  boundary:
    "after one warm save/readback; before each UI edit through matching ACK and fresh context readback",
} as const;
export const workloadHash = createHash("sha256").update(JSON.stringify(workload)).digest("hex");
export const phases = [
  "warm",
  "start",
  ...workload.edits.flatMap((_, i) => [`ack-${String(i)}`, `read-${String(i)}`]),
  "closed",
];
