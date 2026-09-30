import { FVOCI_YDOC_FRAGMENT } from "@fvoci/editor/collab";
import { HocuspocusProvider } from "@hocuspocus/provider";
import {
  computed,
  effectScope,
  markRaw,
  onScopeDispose,
  type ComputedRef,
  type EffectScope,
  type MaybeRefOrGetter,
  shallowRef,
  type ShallowRef,
  toValue,
  watch,
} from "vue";
import * as Y from "yjs";
import {
  blockIdOf,
  CLAIM_RETRY_LIMIT,
  collabStatusOf,
  type CollabPeer,
  type CollabStatus,
  type CollabUser,
  peersEqual,
  peersFromStates,
  persistNow,
  reassertPresence,
  titleEditingOf,
} from "@/features/documents/collab-model";
import {
  type CollabConnectionStatus,
  createConnectionGeneration,
  createPersistAck,
  isDurablySaved,
  type PersistBindState,
  reducePersistBind,
  scopedPersistObserver,
  syncPersistBind,
} from "@/features/documents/collab-persist-ack";
import {
  createRefusalAwareSocket,
  RoomConnection,
  type RefusalAwareSocket,
  type RoomConnectionState,
  type RoomTimers,
} from "@/features/documents/collab-reconnect";

// The Vue app's collab room: one Y.Doc and one HocuspocusProvider per room,
// the counterpart of the React CollabRoom/useCollabSession
// (features/documents/collab-session.tsx) over the same framework-free
// modules. There is no @hocuspocus/provider-vue; this composable is the
// whole binding.
//
// - The Y.Doc, sockets and providers are markRaw and live in shallowRefs:
//   Vue proxies would break Yjs type checks and the identity checks of the
//   persist ACK state (collab-persist-ack.ts compares providers with Object.is).
// - One room per component instance: the caller keys the component by the
//   room name, so moving to another document tears this room down and a new
//   instance builds the next one (never a KeepAlive).
// - The socket layer is RoomConnection (collab-reconnect.ts): a refusal is
//   only recorded (the socket retries with backoff), a failed claim swaps the
//   Y.Doc clientID and opens the next socket generation. Each generation gets
//   its own provider; the previous one is destroyed on a 0 ms timer.
// - Teardown order (as React's layout-effect cleanup, then passive cleanup):
//   flush the batched edits while the socket is open, destroy the socket
//   after 0 ms, then the provider after 0 ms.

const BROWSER_TIMERS: RoomTimers = {
  setTimeout: (callback, ms) => window.setTimeout(callback, ms),
};

/** One socket generation's provider and its session state. */
export interface CollabRoomSession {
  /** Changes only on a reclaim: the editor is keyed on it. */
  readonly generation: number;
  readonly provider: HocuspocusProvider;
  readonly doc: Y.Doc;
  readonly fragment: Y.XmlFragment;
  readonly status: CollabStatus;
  /** The body finished its first sync: only then may the editor mount. */
  readonly synced: boolean;
  /** WHY: #517 — edits the server has not acknowledged on the socket yet. */
  readonly pending: boolean;
  /** WHY: a persist:<id> ACK matched this document, connection and edit prefix. */
  readonly durableSaved: boolean;
  readonly peers: readonly CollabPeer[];
  readonly readOnly: boolean;
  persistNow(): Promise<void>;
}

export interface CollabRoom {
  /** `${workspaceId}:${kind}:${id}` — the Hocuspocus document name. */
  readonly name: string;
  readonly doc: Y.Doc;
  /** The current generation's session; null once the room is disposed. */
  readonly session: ComputedRef<CollabRoomSession | null>;
}

function roomNameOf(provider: HocuspocusProvider): string {
  const name = provider.configuration.name;
  return typeof name === "string" ? name : "";
}

export function collabRoomName(workspaceId: string, kind: "document" | "task", id: string): string {
  return `${workspaceId}:${kind}:${id}`;
}

/**
 * Joins the room `name` for the calling component's lifetime. `user` is the
 * signed-in user's awareness identity; the room connects without it, but
 * presence and awareness wait for it.
 */
export function useCollabRoom(name: string, user: MaybeRefOrGetter<CollabUser | null>): CollabRoom {
  const proto = window.location.protocol === "https:" ? "wss" : "ws";
  const url = `${proto}://${window.location.host}/collab`;
  const doc = markRaw(new Y.Doc({ gc: false }));

  /* WHY: #664 — 서버는 연결이 접속 때 선언한 awareness clientId 하나만 받는다. clientId 는
   * Y.Doc 의 것이라 우리가 만들어 token 으로 넘긴다. #683 — 선언이 거부되면 clientID 를 갈고
   * 소켓 층부터 다시 세운다. Y.Doc 은 살아남아 미전송 편집을 다음 동기화에 싣는다(#704). */
  const room: ShallowRef<RoomConnectionState<RefusalAwareSocket>> = shallowRef(
    undefined as unknown as RoomConnectionState<RefusalAwareSocket>,
  );
  const connection = new RoomConnection<RefusalAwareSocket>({
    open: (onClosed) => markRaw(createRefusalAwareSocket({ url }, onClosed)),
    onChange: (state) => {
      room.value = state;
    },
    /* WHY: #704 — Yjs 도 clientID 충돌을 보면 같은 자리를 이렇게 갈아 낀다. */
    beforeReclaim: () => {
      doc.clientID = new Y.Doc().clientID;
    },
    reclaimLimit: CLAIM_RETRY_LIMIT,
    timers: BROWSER_TIMERS,
  });
  room.value = connection.state;

  interface Generation {
    provider: HocuspocusProvider;
    scope: EffectScope;
    session: ComputedRef<CollabRoomSession>;
  }
  const current = shallowRef<Generation | null>(null);
  let disposed = false;

  function bindGeneration(state: RoomConnectionState<RefusalAwareSocket>): void {
    const previous = current.value;
    /* WHY: #683 — provider 만 갈아끼우면 업스트림 detach 가 방 이름만 보고 지워 옛 연결의
     * 지연 destroy 가 새 연결을 라우팅 맵에서 밀어낸다. 소켓(세대)마다 provider 를 새로 만든다. */
    const provider = markRaw(
      new HocuspocusProvider({
        websocketProvider: state.socket,
        name,
        document: doc,
        token: String(doc.clientID),
        /* WHY: #517 — 배칭이 없으면 타건마다 unsynced 가 +1·−1 로 배지가 스트로브한다. */
        flushDelay: 200,
      }),
    );
    const scope = effectScope(true);
    const session = scope.run(() => {
      const bound = bindSession(provider, state.generation);
      // Registered after the session's own listeners, as the React room's
      // handlers run after its children's: the state machine sees the result
      // last. Removed with the scope, before the provider's delayed destroy.
      const onAuthenticated = () => {
        connection.authenticated();
      };
      const onAuthenticationFailed = () => {
        connection.reclaim();
      };
      provider.on("authenticated", onAuthenticated);
      provider.on("authenticationFailed", onAuthenticationFailed);
      onScopeDispose(() => {
        provider.off("authenticated", onAuthenticated);
        provider.off("authenticationFailed", onAuthenticationFailed);
      });
      return bound;
    });
    if (!session) throw new Error("Collaboration generation scope did not run");
    provider.attach();
    current.value = { provider, scope, session };
    if (previous) retire(previous);
  }

  function retire(generation: Generation): void {
    generation.scope.stop();
    window.setTimeout(() => {
      generation.provider.destroy();
    }, 0);
  }

  function bindSession(
    provider: HocuspocusProvider,
    generation: number,
  ): ComputedRef<CollabRoomSession> {
    const documentId = roomNameOf(provider);
    // WHY: #653 — a provider that already synced must not fold back to "not loaded".
    const synced = shallowRef(provider.synced);
    const unsent = shallowRef(false);
    const readOnly = shallowRef(false);
    const unauthorized = shallowRef(false);
    const peers = shallowRef<CollabPeer[]>([]);
    const connectionStatus = shallowRef<CollabConnectionStatus>(
      provider.configuration.websocketProvider.status,
    );
    const first = createConnectionGeneration(provider, connectionStatus.value);
    const bind = shallowRef<PersistBindState>({
      provider: first.provider,
      status: first.status,
      ack: createPersistAck(documentId, first.connectionId),
    });
    const persistAborts = new Set<AbortController>();

    // Removed with the generation's scope (React: useHocuspocusEvent cleanups).
    interface SessionEvents {
      synced: { state: boolean };
      authenticated: { scope: string };
      authenticationFailed: unknown;
      unsyncedChanges: { number: number };
      status: unknown;
      disconnect: unknown;
    }
    const listen = <K extends keyof SessionEvents>(
      event: K,
      handler: (payload: SessionEvents[K]) => void,
    ) => {
      provider.on(event, handler);
      onScopeDispose(() => provider.off(event, handler));
    };
    listen("synced", ({ state }) => {
      if (state) synced.value = true;
    });
    listen("authenticated", ({ scope }) => {
      readOnly.value = scope === "readonly";
      unauthorized.value = false;
    });
    listen("authenticationFailed", () => {
      unauthorized.value = true;
    });
    listen("unsyncedChanges", ({ number }) => {
      unsent.value = number > 0;
    });
    listen("status", () => {
      connectionStatus.value = provider.configuration.websocketProvider
        .status as CollabConnectionStatus;
    });
    listen("disconnect", () => {
      bind.value = syncPersistBind(bind.value, { provider, status: "disconnected", documentId });
    });

    watch(connectionStatus, (status) => {
      bind.value = syncPersistBind(bind.value, { provider, status, documentId });
    });

    // A new connection generation invalidates the persists still in flight.
    watch(
      () => `${bind.value.ack.documentId}\0${bind.value.ack.connectionId}`,
      () => {
        for (const abort of persistAborts) abort.abort();
        persistAborts.clear();
      },
    );

    const onUpdate = () => {
      bind.value = reducePersistBind(bind.value, { type: "edit" });
    };
    doc.on("update", onUpdate);
    onScopeDispose(() => {
      doc.off("update", onUpdate);
    });

    watch(
      () => toValue(user),
      (next) => {
        if (next) provider.setAwarenessField("user", next);
      },
      { immediate: true },
    );

    watch(
      () => toValue(user),
      (next, _previous, onCleanup) => {
        const awareness = provider.awareness;
        if (!awareness || !next) return;
        let lastBlockId: string | null = null;
        let lastTitleEditing = false;
        const onAwareness = () => {
          const local = awareness.getLocalState();
          if (local !== null) {
            lastBlockId = blockIdOf(local);
            lastTitleEditing = titleEditingOf(local);
          }
          const nextPeers = peersFromStates(awareness.getStates(), awareness.clientID, next.id);
          /* WHY: #571 — 캐럿 이동마다 새 배열을 커밋하면 문서 페이지가 다시 그려진다. */
          if (!peersEqual(peers.value, nextPeers)) peers.value = nextPeers;
        };
        awareness.on("change", onAwareness);
        onAwareness();
        const onPageShow = () => {
          reassertPresence(awareness, next, lastBlockId, lastTitleEditing);
        };
        const onPageHide = () => {
          provider.flushPendingUpdates();
        };
        window.addEventListener("pageshow", onPageShow);
        window.addEventListener("pagehide", onPageHide);
        onCleanup(() => {
          window.removeEventListener("pageshow", onPageShow);
          window.removeEventListener("pagehide", onPageHide);
          awareness.off("change", onAwareness);
        });
      },
      { immediate: true },
    );

    const fragment = markRaw(doc.getXmlFragment(FVOCI_YDOC_FRAGMENT));

    function persist(): Promise<void> {
      const scope = bind.value.ack;
      const abort = new AbortController();
      persistAborts.add(abort);
      return persistNow(
        provider,
        scopedPersistObserver(scope.documentId, scope.connectionId, (event) => {
          bind.value = reducePersistBind(bind.value, event);
        }),
        { signal: abort.signal },
      ).finally(() => {
        persistAborts.delete(abort);
      });
    }

    return computed<CollabRoomSession>(() => ({
      generation,
      provider,
      doc,
      fragment,
      status: collabStatusOf(unauthorized.value, room.value.refusal, connectionStatus.value),
      synced: synced.value,
      // WHY: #517 — readOnly 연결은 서버가 update 에 ack 를 주지 않아 카운터가 내려가지 않는다.
      pending: unsent.value && !readOnly.value,
      durableSaved: isDurablySaved(bind.value.ack),
      peers: peers.value,
      readOnly: readOnly.value,
      persistNow: persist,
    }));
  }

  bindGeneration(connection.state);
  watch(
    () => room.value.generation,
    () => {
      if (!disposed) bindGeneration(room.value);
    },
  );

  onScopeDispose(() => {
    disposed = true;
    // Flush while the socket is open and destroy it after 0 ms (RoomConnection),
    // then the provider after 0 ms, in that order.
    connection.dispose();
    const last = current.value;
    current.value = null;
    if (last) retire(last);
  });

  return {
    name,
    doc,
    session: computed(() => current.value?.session.value ?? null),
  };
}
