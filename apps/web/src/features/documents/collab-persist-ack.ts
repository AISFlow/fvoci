/**
 * Durable save confirmation is not socket sync. Only a matching
 * persist:<id> → persisted:<id> for this document, connection, and
 * edit prefix can show saved. Later edits (including delete-only) and
 * wrong/late/failed/timeout acks cannot.
 */
export interface PersistAckState {
  documentId: string;
  connectionId: string;
  editSeq: number;
  inflight: { requestId: string; prefix: number } | null;
  confirmedPrefix: number | null;
}

export type PersistAckEvent =
  | { type: "bind"; documentId: string; connectionId: string }
  | { type: "edit" }
  | { type: "request"; requestId: string }
  | { type: "ack"; requestId: string }
  | { type: "fail"; requestId: string }
  | { type: "timeout"; requestId: string };

export function createPersistAck(
  documentId: string,
  connectionId: string,
): PersistAckState {
  return {
    documentId,
    connectionId,
    editSeq: 0,
    inflight: null,
    confirmedPrefix: null,
  };
}

export function applyPersistAck(
  state: PersistAckState,
  event: PersistAckEvent,
): PersistAckState {
  switch (event.type) {
    case "bind":
      if (
        event.documentId === state.documentId &&
        event.connectionId === state.connectionId
      ) {
        return state;
      }
      return createPersistAck(event.documentId, event.connectionId);
    case "edit":
      return { ...state, editSeq: state.editSeq + 1 };
    case "request":
      return {
        ...state,
        inflight: { requestId: event.requestId, prefix: state.editSeq },
      };
    case "ack": {
      const inflight = state.inflight;
      if (!inflight || inflight.requestId !== event.requestId) return state;
      if (inflight.prefix !== state.editSeq) {
        return { ...state, inflight: null };
      }
      return {
        ...state,
        inflight: null,
        confirmedPrefix: inflight.prefix,
      };
    }
    case "fail":
    case "timeout":
      if (!state.inflight || state.inflight.requestId !== event.requestId) {
        return state;
      }
      return { ...state, inflight: null };
  }
}

export function isDurablySaved(state: PersistAckState): boolean {
  return (
    state.confirmedPrefix !== null && state.confirmedPrefix === state.editSeq
  );
}
