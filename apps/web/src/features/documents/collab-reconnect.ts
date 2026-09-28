/* WHY: 방 수용 한도(close 1013)나 협업 불가(1011)로 서버가 인증 전에 소켓을 닫으면 provider 4.6.0
 * 은 두 경로로 동시에 재접속한다. onOpen 이 재시도 루프의 취소 핸들을 지운 뒤라
 * (hocuspocus-provider.esm.js:188-193) 루프는 첫 서버 프레임 전까지 끝나지 않았는데도
 * (:305-334) onClose 가 setTimeout(connect) 로 새 루프를 만들고(:404-414) 옛 루프도 자기
 * backoff 로 계속 돈다. 거절될 때마다 루프가 하나씩 늘어 간격이 1 s → 20 ms 로 줄어드는
 * 폭주가 된다. OwnedSocket 이 취소 핸들을 첫 프레임까지 쥐고 있어 루프는 늘 하나다: 같은
 * 소켓이 상한 있는 jitter backoff(RECONNECT_BACKOFF)로 다시 열고 열 때마다 인증을 다시
 * 보낸다. 거절은 사유만 기록한다 — 소켓·provider·화면을 갈아끼우지 않는다. 기록은 close 마다
 * 갱신한다: 열리지도 못한 시도(서버 다운·오프라인·업그레이드 503)는 거절이 아니므로 사유를 지워
 * 상태가 원래 연결 상태로 돌아간다 — 장애 내내 「서버 혼잡」이 남지 않는다. */

import {
	HocuspocusProviderWebsocket,
	type HocuspocusProviderWebsocketConfiguration,
} from "@hocuspocus/provider";

/** Backoff of the socket's own retry loop (@lifeomic/attempt 3.1 via provider 4.6.0): retry n
 * waits a random delay in [minDelay, min(delay·factor^(n-1), maxDelay)]. minDelay < delay keeps
 * the first retry jittered; delay < minDelay would make connect() reject. A served session that
 * drops waits `delay` before its fresh loop starts again from the first step. */
export const RECONNECT_BACKOFF = {
	delay: 1_000,
	minDelay: 500,
	maxDelay: 10_000,
	factor: 2,
	jitter: true,
} as const;

/** Server close code for a capacity refusal ("try again later"). */
export const CLOSE_TRY_AGAIN_LATER = 1013;

export type CollabRefusal = "capacity" | "unavailable";

/** A socket that opened but was closed before any server frame is a refusal, not a dropped session. */
export function refusalOf(
	openedWithoutFrame: boolean,
	code: number | undefined,
): CollabRefusal | null {
	if (!openedWithoutFrame) return null;
	return code === CLOSE_TRY_AGAIN_LATER ? "capacity" : "unavailable";
}

/** Tracks one socket generation: did it open, and did the server send anything since. */
export class RefusalWatch {
	private opened = false;
	private framed = false;

	open(): void {
		this.opened = true;
		this.framed = false;
	}

	frame(): void {
		this.framed = true;
	}

	/** Returns the refusal for this close and resets for the next attempt. */
	close(code: number | undefined): CollabRefusal | null {
		const refusal = refusalOf(this.opened && !this.framed, code);
		this.opened = false;
		this.framed = false;
		return refusal;
	}
}

/* WHY: 두 가지를 provider 4.6.0 위에서 고친다(공개 멤버만 재정의, index.d.ts:261-281).
 * 1) 재시도 루프는 하나: onOpen 은 취소 핸들을 지우지 않고, 첫 서버 프레임이 시도를 끝낼 때만
 *    지운다. 그 전의 close 는 루프가 backoff 로 다시 시도하고, onClose 는 핸들이 있어 새 루프를
 *    만들지 않는다. 서비스 중 끊긴 세션(프레임 뒤)은 핸들이 없어 onClose 가 새 루프를 연다.
 * 2) destroy() 는 이미 걸린 onClose 의 setTimeout(connect) 를 지우지 못한다. 그 connect() 가
 *    shouldConnect 를 다시 켜서 파기된 인스턴스가 리스너 없는 소켓을 연다(좀비). 파기 뒤 connect 는 무시한다. */
class OwnedSocket extends HocuspocusProviderWebsocket {
	private destroyed = false;

	onOpen(event: Event): Promise<void> {
		const loop = this.cancelWebsocketRetry;
		const opened = super.onOpen(event);
		/* super.onOpen has no await before it clears the handle, so this restore is in time. */
		this.cancelWebsocketRetry = loop;
		return opened;
	}

	resolveConnectionAttempt(): void {
		if (this.connectionAttempt) this.cancelWebsocketRetry = undefined;
		super.resolveConnectionAttempt();
	}

	connect(): Promise<unknown> {
		if (this.destroyed) return Promise.resolve();
		return super.connect();
	}

	destroy(): void {
		this.destroyed = true;
		super.destroy();
	}
}

/** The room's socket. It retries refusals itself with RECONNECT_BACKOFF (callers may
 * override the timing) and reports every close to `onClosed`, which only records it: the
 * refusal, or null for a close that was not one (a served session that dropped, or an
 * attempt that never opened), so a stale refusal never outlives the next close. */
export function createRefusalAwareSocket(
	configuration: HocuspocusProviderWebsocketConfiguration,
	onClosed: (refusal: CollabRefusal | null) => void,
): HocuspocusProviderWebsocket {
	const watch = new RefusalWatch();
	return new OwnedSocket({
		...RECONNECT_BACKOFF,
		...configuration,
		onOpen: () => watch.open(),
		onMessage: () => watch.frame(),
		onClose: ({ event }) => onClosed(watch.close(event?.code)),
	});
}

/** Timers the room controller defers socket teardown with (injected so tests drive them). */
export interface RoomTimers {
	setTimeout(callback: () => void, ms: number): unknown;
}

/** What the controller touches on a socket generation. */
export interface RoomSocketHandle {
	readonly configuration: {
		readonly providerMap: ReadonlyMap<string, { flushPendingUpdates(): void }>;
	};
	destroy(): void;
}

export interface RoomConnectionState<S> {
	/** The room's socket; it retries refusals on its own. */
	readonly socket: S;
	/** Changes only on a reclaim (#683): consumers remount on it, never on a refusal. */
	readonly generation: number;
	/** Pre-auth refusal of the latest close; cleared by a close that was not a refusal
	 * (e.g. an attempt that never opened), by authenticating, or by a reclaim. */
	readonly refusal: CollabRefusal | null;
}

export interface RoomConnectionOptions<S extends RoomSocketHandle> {
	/** Opens a socket that reports every close: its refusal, or null for any other close. */
	open(onClosed: (refusal: CollabRefusal | null) => void): S;
	onChange(state: RoomConnectionState<S>): void;
	/** Runs before a reclaim opens the next socket: swap the Y.Doc clientID here (#683/#704). */
	beforeReclaim(): void;
	reclaimLimit: number;
	timers: RoomTimers;
}

/** One collab room's connection state machine, free of React.
 * - A refusal is recorded only; the socket's own loop retries (no new socket, no remount).
 *   A later close that is not a refusal clears it, so the raw connection status shows again.
 * - authenticated() clears the refusal and refills the reclaim budget.
 * - reclaim() (authenticationFailed) swaps the clientID and opens the next socket generation,
 *   at most `reclaimLimit` times between authentications.
 * - dispose() and every replaced socket flush the attached rooms' batched edits while the
 *   socket is still open, then destroy it one task later; later events are ignored. */
export class RoomConnection<S extends RoomSocketHandle> {
	private readonly options: RoomConnectionOptions<S>;
	private current: RoomConnectionState<S>;
	private reclaims = 0;
	private disposed = false;

	constructor(options: RoomConnectionOptions<S>) {
		this.options = options;
		this.current = { socket: this.open(0), generation: 0, refusal: null };
	}

	get state(): RoomConnectionState<S> {
		return this.current;
	}

	authenticated(): void {
		if (this.disposed) return;
		this.reclaims = 0;
		this.record(null);
	}

	/** Returns false when disposed or the budget is spent (the room stays unauthorized). */
	reclaim(): boolean {
		if (this.disposed || this.reclaims >= this.options.reclaimLimit) return false;
		this.reclaims += 1;
		this.options.beforeReclaim();
		this.release(this.current.socket);
		const generation = this.current.generation + 1;
		this.current = { socket: this.open(generation), generation, refusal: null };
		this.options.onChange(this.current);
		return true;
	}

	dispose(): void {
		if (this.disposed) return;
		this.disposed = true;
		this.release(this.current.socket);
	}

	private open(generation: number): S {
		return this.options.open((refusal) => {
			if (this.disposed || generation !== this.current.generation) return;
			this.record(refusal);
		});
	}

	private record(refusal: CollabRefusal | null): void {
		if (this.current.refusal === refusal) return;
		this.current = { ...this.current, refusal };
		this.options.onChange(this.current);
	}

	/* WHY: HocuspocusRoom destroys its provider from a passive-effect cleanup on a 0 ms timer that
	 * fires after this one, when the socket is already gone: its flushPendingUpdates would only
	 * queue the last ≤200 ms batch on a dead socket, and on unmount the Y.Doc is dropped with it.
	 * Flush here, while the socket is still open. On a reclaim the flush is moot (a closed socket
	 * only queues it; an unauthenticated one is ignored by the server, transport.rs Denied), and
	 * nothing is lost: that Y.Doc survives and the next socket's sync carries the edits. */
	private release(socket: S): void {
		for (const provider of socket.configuration.providerMap.values()) {
			provider.flushPendingUpdates();
		}
		this.options.timers.setTimeout(() => socket.destroy(), 0);
	}
}
