import type { CollabStatus } from "./collab-model";

export type CollabBadgeTone = "live" | "wait" | "danger";

export interface CollabBadge {
  label:
    | "doc.collab.saved"
    | "doc.collab.connected"
    | "doc.collab.pending"
    | "doc.collab.connecting"
    | "doc.collab.reconnecting"
    | "doc.collab.unauthorized"
    | "doc.collab.busy"
    | "doc.collab.unavailable";
  tone: CollabBadgeTone;
}

const COLLAB_BADGE: Record<CollabStatus, CollabBadge> = {
  connected: {
    label: "doc.collab.connected",
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
  busy: {
    label: "doc.collab.busy",
    tone: "danger",
  },
  unavailable: {
    label: "doc.collab.unavailable",
    tone: "danger",
  },
};

/** Body note for a room the server refused: the body is not loaded, not empty. */
export function collabRefusalNote(
  status: CollabStatus | undefined,
): "doc.collab.busyNote" | "doc.collab.unavailableNote" | null {
  if (status === "busy") return "doc.collab.busyNote";
  if (status === "unavailable") return "doc.collab.unavailableNote";
  return null;
}

/** WHY: 소켓 동기화와 DB persist ack 는 다르다. 「저장됨」은 일치하는 persist 성공만. */
export function collabBadge(
  status: CollabStatus,
  pending: boolean,
  persisted = false,
): CollabBadge {
  if (status !== "connected") return COLLAB_BADGE[status];
  if (pending) {
    return {
      label: "doc.collab.pending",
      tone: "wait",
    };
  }
  if (persisted) {
    return {
      label: "doc.collab.saved",
      tone: "live",
    };
  }
  return COLLAB_BADGE.connected;
}
