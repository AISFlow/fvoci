import type { CollabStatus } from "./collab-model";

export type CollabBadgeTone = "live" | "wait" | "danger";

export interface CollabBadge {
  label:
    | "doc.collab.saved"
    | "doc.collab.pending"
    | "doc.collab.connecting"
    | "doc.collab.reconnecting"
    | "doc.collab.unauthorized";
  tone: CollabBadgeTone;
}

const COLLAB_BADGE: Record<CollabStatus, CollabBadge> = {
  connected: {
    label: "doc.collab.saved",
    tone: "live",
  },
  connecting: {
    label: "doc.collab.connecting",
    tone: "wait",
  },
  disconnected: {
    label: "doc.collab.reconnecting",
    tone: "wait",
  },
  unauthorized: {
    label: "doc.collab.unauthorized",
    tone: "danger",
  },
};

/** WHY: #517 — 오프라인이어도 provider 는 30초까지 connected 다. 「저장됨」은 미전송 변경이 없을 때만. */
export function collabBadge(status: CollabStatus, pending: boolean): CollabBadge {
  return status === "connected" && pending
    ? {
        label: "doc.collab.pending",
        tone: "wait",
      }
    : COLLAB_BADGE[status];
}
