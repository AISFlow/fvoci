/**
 * Durable save confirmation is not socket sync. Only a matching
 * persist:<id> → persisted:<id> for this document, connection, and
 * edit prefix can show saved. Later edits (including delete-only) and
 * wrong/late/failed/timeout acks cannot.
 *
 * A Hocuspocus provider object often survives reconnect. Connection
 * generation must rotate on disconnect (not provider identity alone)
 * so a delayed persist callback from the previous socket cannot mutate
 * the rebound room.
 */
export interface PersistAckState {
  documentId: string;
  connectionId: string;
  editSeq: number;
  inflight: { requestId: string; prefix: number } | null;
  confirmedPrefix: number | null;
}

export type PersistAckScope = {
  documentId: string;
  connectionId: string;
};

export type PersistAckEvent =
  | ({ type: "bind" } & PersistAckScope)
  | { type: "edit" }
  | ({ type: "request"; requestId: string } & PersistAckScope)
  | ({ type: "ack"; requestId: string } & PersistAckScope)
  | ({ type: "fail"; requestId: string } & PersistAckScope)
  | ({ type: "timeout"; requestId: string } & PersistAckScope);

export type CollabConnectionStatus =
  | "connecting"
  | "connected"
  | "disconnected";

export interface CollabConnectionGeneration {
  provider: unknown;
  status: CollabConnectionStatus;
  connectionId: string;
}

export interface PersistBindState {
  provider: unknown;
  status: CollabConnectionStatus;
  ack: PersistAckState;
}

export interface ScopedPersistObserver {
  onRequest: (requestId: string) => void;
  onAck: (requestId: string) => void;
  onFail: (requestId: string) => void;
  onTimeout: (requestId: string) => void;
}

function cryptoRandomId(): string {
  return crypto.randomUUID();
}

function sameScope(state: PersistAckState, scope: PersistAckScope): boolean {
  return (
    state.documentId === scope.documentId &&
    state.connectionId === scope.connectionId
  );
}

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

export function createConnectionGeneration(
  provider: unknown,
  status: CollabConnectionStatus,
  nextId: () => string = cryptoRandomId,
): CollabConnectionGeneration {
  return { provider, status, connectionId: nextId() };
}

/**
 * Rotate when the provider instance changes or the same provider leaves
 * a live socket. connecting→connected on that instance keeps the id so
 * the first persist is not wiped by the open handshake.
 */
export function advanceConnectionGeneration(
  current: CollabConnectionGeneration,
  next: Pick<CollabConnectionGeneration, "provider" | "status">,
  nextId: () => string = cryptoRandomId,
): CollabConnectionGeneration {
  const providerChanged = !Object.is(next.provider, current.provider);
  const enteredDisconnected =
    next.status === "disconnected" && current.status !== "disconnected";
  const leftConnected =
    current.status === "connected" && next.status !== "connected";
  if (providerChanged || enteredDisconnected || leftConnected) {
    return {
      provider: next.provider,
      status: next.status,
      connectionId: nextId(),
    };
  }
  return {
    provider: next.provider,
    status: next.status,
    connectionId: current.connectionId,
  };
}

export function syncPersistBind(
  state: PersistBindState,
  next: {
    provider: unknown;
    status: CollabConnectionStatus;
    documentId: string;
  },
  nextId: () => string = cryptoRandomId,
): PersistBindState {
  const generation = advanceConnectionGeneration(
    {
      provider: state.provider,
      status: state.status,
      connectionId: state.ack.connectionId,
    },
    { provider: next.provider, status: next.status },
    nextId,
  );
  const ack = applyPersistAck(state.ack, {
    type: "bind",
    documentId: next.documentId,
    connectionId: generation.connectionId,
  });
  if (
    Object.is(state.provider, generation.provider) &&
    state.status === generation.status &&
    ack === state.ack
  ) {
    return state;
  }
  return {
    provider: generation.provider,
    status: generation.status,
    ack,
  };
}

export function reducePersistBind(
  state: PersistBindState,
  event: PersistAckEvent,
): PersistBindState {
  const ack = applyPersistAck(state.ack, event);
  return ack === state.ack ? state : { ...state, ack };
}

/** Snapshot room+connection at persist request time; delayed callbacks keep that generation. */
export function scopedPersistObserver(
  documentId: string,
  connectionId: string,
  dispatch: (event: PersistAckEvent) => void,
): ScopedPersistObserver {
  const scope: PersistAckScope = { documentId, connectionId };
  return {
    onRequest: (requestId) =>
      dispatch({ type: "request", requestId, ...scope }),
    onAck: (requestId) => dispatch({ type: "ack", requestId, ...scope }),
    onFail: (requestId) => dispatch({ type: "fail", requestId, ...scope }),
    onTimeout: (requestId) =>
      dispatch({ type: "timeout", requestId, ...scope }),
  };
}

export function applyPersistAck(
  state: PersistAckState,
  event: PersistAckEvent,
): PersistAckState {
  switch (event.type) {
    case "bind":
      if (sameScope(state, event)) {
        return state;
      }
      return createPersistAck(event.documentId, event.connectionId);
    case "edit":
      return { ...state, editSeq: state.editSeq + 1 };
    case "request":
      if (!sameScope(state, event)) return state;
      return {
        ...state,
        inflight: { requestId: event.requestId, prefix: state.editSeq },
      };
    case "ack": {
      if (!sameScope(state, event)) return state;
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
      if (!sameScope(state, event)) return state;
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
