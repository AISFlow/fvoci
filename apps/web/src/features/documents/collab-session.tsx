import { FVOCI_YDOC_FRAGMENT } from "@fvoci/editor/collab";
import {
	HocuspocusProviderWebsocketComponent,
	HocuspocusRoom,
	useHocuspocusConnectionStatus,
	useHocuspocusEvent,
	useHocuspocusProvider,
} from "@hocuspocus/provider-react";
import {
	createContext,
	type ReactNode,
	useContext,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import * as Y from "yjs";
import {
	createConnectionGeneration,
	createPersistAck,
	isDurablySaved,
	reducePersistBind,
	scopedPersistObserver,
	syncPersistBind,
	type PersistBindState,
} from "./collab-persist-ack";
import {
	blockIdOf,
	CLAIM_RETRY_LIMIT,
	collabStatusOf,
	type CollabPeer,
	type CollabSession,
	type CollabStatus,
	type CollabUser,
	persistNow,
	peersEqual,
	peersFromStates,
	reassertPresence,
	titleEditingOf,
} from "./collab-model";
import {
	type CollabRefusal,
	createRefusalAwareSocket,
	type RefusalAwareSocket,
	RoomConnection,
	type RoomConnectionState,
	type RoomTimers,
} from "./collab-reconnect";

export type { CollabPeer, CollabSession, CollabStatus, CollabUser };
export {
	CLAIM_RETRY_LIMIT,
	collabUserOf,
	PERSIST_TIMEOUT_MS,
	persistNow,
	peersEqual,
	peersFromStates,
	reassertPresence,
	setTitleEditing,
} from "./collab-model";
export { isDurablySaved } from "./collab-persist-ack";

/** Pre-auth refusal of this room socket's latest close, cleared by a close that was not a refusal
 * or once the room authenticates. */
const CollabRefusalContext = createContext<CollabRefusal | null>(null);

function roomNameOf(provider: { configuration?: { name?: string } }): string {
	const name = provider.configuration?.name;
	return typeof name === "string" ? name : "";
}

export function CollabRoom({
	workspaceId,
	kind = "document",
	id,
	children,
}: {
	workspaceId: string;
	kind?: "document" | "task";
	id: string;
	children: ReactNode;
}) {
	const name = `${workspaceId}:${kind}:${id}`;
	/* WHY: #664 — 방을 옮기면 Y.Doc·선언 예산이 전부 새로 나야 한다. key 로 갈아끼운다. */
	return (
		<ClaimedRoom key={name} name={name}>
			{children}
		</ClaimedRoom>
	);
}

/* WHY: #664 — 서버는 연결이 접속 때 선언한 awareness clientId 하나만 받는다(선언 없으면 거절).
 * clientId 는 Y.Doc 의 것이라 provider 에 맡기면 접속 뒤에야 알 수 있다 — 우리가 만들어 넘긴다.
 * #683 — 선언이 거부되면 clientID 를 갈고 소켓 층부터 다시 세운다. Y.Doc 은 이 층에 있어 살아남고
 * (#704 미전송 편집 보존), Awareness 는 provider 가 새로 만들어 새 id 로 굳는다.
 * 인증 전 거절(방 한도 1013 등)은 소켓이 같은 자리에서 backoff 로 다시 연다 — 사유만 기록하고
 * 아래 페이지(초안·포커스·불러온 편집기)는 그대로 둔다. 상태 기계는 RoomConnection(collab-reconnect.ts). */
function ClaimedRoom({
	name,
	children,
}: {
	name: string;
	children: ReactNode;
}) {
	const proto = window.location.protocol === "https:" ? "wss" : "ws";
	const url = `${proto}://${window.location.host}/collab`;
	const [doc] = useState(() => new Y.Doc({ gc: false }));
	const [room, setRoom] = useState<RoomConnectionState<RefusalAwareSocket> | null>(null);
	const connection = useRef<RoomConnection<RefusalAwareSocket> | null>(null);
	/* Built in a layout effect so StrictMode's double run cannot leave a connected socket behind. */
	useLayoutEffect(() => {
		const next = new RoomConnection<RefusalAwareSocket>({
			open: (onClosed) => createRefusalAwareSocket({ url }, onClosed),
			onChange: setRoom,
			/* WHY: #704 — Yjs 도 clientID 충돌을 보면 같은 자리를 이렇게 갈아 낀다(yjs.mjs:3342).
			 * 구조체는 옛 id 통에 그대로 남아 다음 동기화에 실린다 — 미전송 편집이 살아남는다. */
			beforeReclaim: () => {
				doc.clientID = new Y.Doc().clientID;
			},
			reclaimLimit: CLAIM_RETRY_LIMIT,
			timers: BROWSER_TIMERS,
		});
		connection.current = next;
		setRoom(next.state);
		return () => {
			if (connection.current === next) connection.current = null;
			next.dispose();
		};
	}, [url, doc]);
	if (room === null) return null;
	return (
		/* WHY: #683 — provider 만 갈아끼우면 업스트림 detach 가 방 이름만 보고 지워(provider 4.6.0
		 * hocuspocus-provider.esm.js:204-208) 옛 연결의 지연 destroy 가 새 연결을 라우팅 맵에서
		 * 밀어낸다. providerMap 은 소켓마다 따로라 재선언은 소켓째 갈아끼운다(세대 key). 거절은
		 * 세대를 바꾸지 않는다. */
		<HocuspocusProviderWebsocketComponent key={room.generation} websocketProvider={room.socket}>
			<HocuspocusRoom
				name={name}
				document={doc}
				token={String(doc.clientID)}
				/* WHY: #517 — 배칭이 없으면 타건마다 unsynced 가 +1·−1 로 배지가 스트로브한다. */
				flushDelay={200}
				onAuthenticated={() => connection.current?.authenticated()}
				onAuthenticationFailed={() => connection.current?.reclaim()}
			>
				<CollabRefusalContext.Provider value={room.refusal}>{children}</CollabRefusalContext.Provider>
			</HocuspocusRoom>
		</HocuspocusProviderWebsocketComponent>
	);
}

const BROWSER_TIMERS: RoomTimers = {
	setTimeout: (callback, ms) => window.setTimeout(callback, ms),
};

export function useCollabSession(
	user: CollabUser | null,
): CollabSession | null {
	const provider = useHocuspocusProvider();
	const connectionStatus = useHocuspocusConnectionStatus();
	const refusal = useContext(CollabRefusalContext);
	const documentId = roomNameOf(provider);
	// WHY: #653 — provider 가 살아있는 채 리마운트되면 synced 를 다시 미동기화로 접으면 안 된다.
	const [synced, setSynced] = useState(() => provider.synced);
	const [unsent, setUnsent] = useState(false);
	const [peers, setPeers] = useState<CollabPeer[]>([]);
	const [readOnly, setReadOnly] = useState(false);
	const [unauthorized, setUnauthorized] = useState(false);
	const [bind, setBind] = useState<PersistBindState>(() => {
		const generation = createConnectionGeneration(provider, connectionStatus);
		return {
			provider: generation.provider,
			status: generation.status,
			ack: createPersistAck(documentId, generation.connectionId),
		};
	});
	const bindRef = useRef(bind);
	bindRef.current = bind;
	const persistAbortsRef = useRef(new Set<AbortController>());
	const ack = bind.ack;
	const generationKey = `${ack.documentId}\0${ack.connectionId}`;
	const generationKeyRef = useRef(generationKey);

	useHocuspocusEvent("synced", ({ state }) => {
		if (state) setSynced(true);
	});
	useHocuspocusEvent("authenticated", ({ scope }) => {
		setReadOnly(scope === "readonly");
		setUnauthorized(false);
	});
	useHocuspocusEvent("authenticationFailed", () => {
		setUnauthorized(true);
	});
	useHocuspocusEvent("unsyncedChanges", ({ number }) => {
		setUnsent(number > 0);
	});

	useEffect(() => {
		setBind((prev) =>
			syncPersistBind(prev, { provider, status: connectionStatus, documentId }),
		);
	}, [provider, connectionStatus, documentId]);

	useEffect(() => {
		if (generationKeyRef.current === generationKey) return;
		generationKeyRef.current = generationKey;
		for (const abort of persistAbortsRef.current) abort.abort();
		persistAbortsRef.current.clear();
	}, [generationKey]);

	useHocuspocusEvent("disconnect", () => {
		setBind((prev) =>
			syncPersistBind(prev, {
				provider,
				status: "disconnected",
				documentId: roomNameOf(provider),
			}),
		);
	});

	useEffect(() => {
		const doc = provider.document;
		const onUpdate = () => {
			setBind((prev) => reducePersistBind(prev, { type: "edit" }));
		};
		doc.on("update", onUpdate);
		return () => {
			doc.off("update", onUpdate);
		};
	}, [provider.document]);

	useEffect(() => {
		if (!user) return;
		provider.setAwarenessField("user", user);
	}, [provider, user]);

	useEffect(() => {
		const awareness = provider.awareness;
		if (!awareness || !user) return;
		let lastBlockId: string | null = null;
		let lastTitleEditing = false;
		const onAwareness = () => {
			const local = awareness.getLocalState();
			if (local !== null) {
				lastBlockId = blockIdOf(local);
				lastTitleEditing = titleEditingOf(local);
			}
			const next = peersFromStates(
				awareness.getStates(),
				awareness.clientID,
				user.id,
			);
			setPeers((prev) => (peersEqual(prev, next) ? prev : next));
		};
		awareness.on("change", onAwareness);
		onAwareness();
		const onPageShow = () => {
			reassertPresence(awareness, user, lastBlockId, lastTitleEditing);
		};
		const onPageHide = () => {
			provider.flushPendingUpdates();
		};
		window.addEventListener("pageshow", onPageShow);
		window.addEventListener("pagehide", onPageHide);
		return () => {
			window.removeEventListener("pageshow", onPageShow);
			window.removeEventListener("pagehide", onPageHide);
			awareness.off("change", onAwareness);
		};
	}, [provider, user]);

	/* WHY: #571 — 렌더마다 새 리터럴을 돌려주면 소비자의 memo 가 전부 무력해진다.
	 * 훅 규칙상 useMemo 는 user 분기 위에 두고 반환만 분기한다. */
	const session = useMemo<CollabSession>(
		() => ({
			provider,
			doc: provider.document,
			fragment: provider.document.getXmlFragment(FVOCI_YDOC_FRAGMENT),
			status: collabStatusOf(unauthorized, refusal, connectionStatus),
			synced,
			// WHY: #517 — readOnly 연결은 서버가 update 에 ack 를 주지 않아 카운터가 내려가지 않는다.
			pending: unsent && !readOnly,
			durableSaved: isDurablySaved(ack),
			peers,
			readOnly,
			persistNow: () => {
				const scope = bindRef.current.ack;
				const abort = new AbortController();
				persistAbortsRef.current.add(abort);
				return persistNow(
					provider,
					scopedPersistObserver(
						scope.documentId,
						scope.connectionId,
						(event) => {
							setBind((prev) => reducePersistBind(prev, event));
						},
					),
					{ signal: abort.signal },
				).finally(() => {
					persistAbortsRef.current.delete(abort);
				});
			},
		}),
		[provider, unauthorized, refusal, connectionStatus, synced, unsent, readOnly, peers, ack],
	);

	if (!user) return null;
	return session;
}
