import {
	COLLAB_PERSIST_DONE,
	COLLAB_PERSIST_FAILED,
	COLLAB_PERSIST_REQUEST,
} from "@fvoci/editor/collab";
import { presenceColorOf } from "../../lib/presence.ts";
import type {
	HocuspocusProvider,
	onStatelessParameters,
} from "@hocuspocus/provider";
import type { Awareness } from "y-protocols/awareness";
import type * as Y from "yjs";

export type CollabStatus =
	| "connecting"
	| "connected"
	| "disconnected"
	| "unauthorized";

export interface CollabUser {
	[key: string]: string;
	id: string;
	name: string;
	color: string;
}

/** WHY: #510 — 탭(clientId) 단위. 헤더는 id 로 묶고 배지는 탭마다 하나. */
export interface CollabPeer {
	clientId: number;
	id: string;
	name: string;
	color: string;
	blockId: string | null;
	/** WHY: #556 B — 제목 칸 포커스. 제목 글자는 REST 정본이라 awareness 에 안 싣는다. */
	titleEditing: boolean;
	self: boolean;
}

function str(value: unknown, key: string): string | undefined {
	if (typeof value !== "object" || value === null) return undefined;
	const v = Reflect.get(value, key);
	return typeof v === "string" && v.length > 0 ? v : undefined;
}

export function blockIdOf(state: unknown): string | null {
	if (typeof state !== "object" || state === null) return null;
	return str(Reflect.get(state, "block"), "id") ?? null;
}

export function titleEditingOf(state: unknown): boolean {
	if (typeof state !== "object" || state === null) return false;
	return Reflect.get(state, "title") === true;
}

/** WHY: #556 B — 제목 칸 포커스만 알린다. 문자열이 오면 서버가 떨어뜨린다. */
export function setTitleEditing(
	awareness: Pick<Awareness, "setLocalStateField">,
	editing: boolean,
): void {
	awareness.setLocalStateField("title", editing ? true : null);
}

/* WHY: #517 H4 — provider 는 pagehide 에서 removeAwarenessStates 로 로컬 상태를 지우고 되돌리지 않는다.
 * 상태가 null 이면 setLocalStateField 는 아무것도 하지 않으므로 전체를 다시 넣어야 한다. 캐럿 플러그인은
 * 마지막 블록과 같으면 다시 쓰지 않으니 block·title 도 함께 복원한다. */
export function reassertPresence(
	awareness: Awareness,
	user: CollabUser,
	blockId: string | null,
	titleEditing = false,
): void {
	if (awareness.getLocalState() !== null) return;
	const state: Record<string, unknown> = { user };
	if (blockId !== null) state.block = { id: blockId };
	if (titleEditing) state.title = true;
	awareness.setLocalState(state);
}

export function peersFromStates(
	states: Map<number, unknown>,
	selfClientId: number,
	selfUserId: string,
): CollabPeer[] {
	const peers: CollabPeer[] = [];
	for (const [clientId, state] of states) {
		if (clientId === selfClientId) continue;
		if (typeof state !== "object" || state === null) continue;
		const user = Reflect.get(state, "user");
		const id = str(user, "id");
		const name = str(user, "name");
		const color = str(user, "color");
		if (id === undefined || name === undefined || color === undefined) continue;
		if (!/^#[0-9a-fA-F]{6}$/.test(color)) continue;
		peers.push({
			clientId,
			id,
			name,
			color,
			/* WHY: 명세 §4 — cursor 가 없으면(포커스 없음) 블록만으로는 「커서 없음」. */
			blockId: Reflect.get(state, "cursor") == null ? null : blockIdOf(state),
			titleEditing: titleEditingOf(state),
			self: id === selfUserId,
		});
	}
	return peers;
}

/* WHY: #571 — awareness 는 로컬 캐럿 이동에도 change 를 쏘고 peersFromStates 는 매번 새 배열을
 * 만든다. 같은 집합이면 상태를 커밋하지 않아야 캐럿 이동이 문서 페이지를 재렌더하지 않는다.
 * 인덱스 비교로 충분하다 — states 는 Map 이라 순서가 유지되고, 순서가 흔들리면 false 로 떨어져
 * 커밋할 뿐 결과가 틀리지는 않는다. */
export function peersEqual(
	a: readonly CollabPeer[],
	b: readonly CollabPeer[],
): boolean {
	if (a.length !== b.length) return false;
	return a.every((peer, index) => {
		const other = b[index];
		return (
			other !== undefined &&
			peer.clientId === other.clientId &&
			peer.id === other.id &&
			peer.name === other.name &&
			peer.color === other.color &&
			peer.blockId === other.blockId &&
			peer.titleEditing === other.titleEditing &&
			peer.self === other.self
		);
	});
}

/* WHY: #641 — 서버가 awareness 의 name·color 를 세션 값으로 덮어쓴다. 같은 순수 함수를 봐야
 * 내 배지 색이 남들이 보는 색과 어긋나지 않는다. */
export interface CollabSession {
	provider: HocuspocusProvider;
	doc: Y.Doc;
	fragment: Y.XmlFragment;
	status: CollabStatus;
	synced: boolean;
	/** WHY: #517 — 서버가 아직 소켓으로 받았다고 답하지 않은 변경이 남았다. */
	pending: boolean;
	/** WHY: persist:<id> 성공 ack 가 이 문서·연결·편집 prefix 와 맞을 때만 true. */
	durableSaved: boolean;
	peers: CollabPeer[];
	readOnly: boolean;
	persistNow: () => Promise<void>;
}

export function collabUserOf(userId: string, name: string): CollabUser {
	return { id: userId, name, color: presenceColorOf(userId) };
}

/* WHY: #683 — 예약 TTL(60s)을 넘는 단절 뒤 남이 그 clientId 를 잡으면, 마운트에 고정된 선언은
 * 영구 거절이 된다. 새 clientID 로 다시 선언하면 잠금이 풀린다. 서버는 예약 충돌도 세션 만료도
 * 같은 permission-denied 로 보내 구분할 수 없으므로 상한을 두고, 인증이 한 번 성공해야 예산이
 * 되채워진다 — 권한을 잃은 사용자는 상한에서 멈춰 unauthorized 배지로 끝난다(재접속 버스트 없음). */
export const CLAIM_RETRY_LIMIT = 3;

export const PERSIST_TIMEOUT_MS = 5000;

export interface PersistNowObserver {
	onRequest?: (requestId: string) => void;
	onAck?: (requestId: string) => void;
	onFail?: (requestId: string) => void;
	onTimeout?: (requestId: string) => void;
}

/* WHY: 저장 응답은 요청별로 확인한다. timeout·실패 응답은 저장 성공이 아니므로
 * 보관·내보내기·버전 저장 호출자가 작업을 중단하고 오류를 표시한다. */
export function persistNow(
	provider: HocuspocusProvider,
	observer?: PersistNowObserver,
): Promise<void> {
	const requestId = crypto.randomUUID();
	return new Promise((resolve, reject) => {
		let settled = false;
		const done = (kind: "ack" | "fail" | "timeout", error?: Error) => {
			if (settled) return;
			settled = true;
			globalThis.clearTimeout(timer);
			provider.off("stateless", onStateless);
			if (kind === "ack") observer?.onAck?.(requestId);
			else if (kind === "fail") observer?.onFail?.(requestId);
			else observer?.onTimeout?.(requestId);
			if (error) reject(error);
			else resolve();
		};
		const onStateless = ({ payload }: onStatelessParameters) => {
			if (payload === `${COLLAB_PERSIST_DONE}:${requestId}`) done("ack");
			if (payload === `${COLLAB_PERSIST_FAILED}:${requestId}`) {
				done("fail", new Error("collab persist failed"));
			}
		};
		const timer = globalThis.setTimeout(() => {
			done("timeout", new Error("collab persist timed out"));
		}, PERSIST_TIMEOUT_MS);
		provider.on("stateless", onStateless);
		/* WHY: flushDelay 배칭은 문서 업데이트만 묶고 stateless 는 직행이라 마지막 ≤200ms 편집을 추월한다 — 먼저 내보낸다. */
		try {
			provider.flushPendingUpdates();
			observer?.onRequest?.(requestId);
			provider.sendStateless(`${COLLAB_PERSIST_REQUEST}:${requestId}`);
		} catch (error) {
			done(
				"fail",
				error instanceof Error ? error : new Error("collab persist failed"),
			);
		}
	});
}
