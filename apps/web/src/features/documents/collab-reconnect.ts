/* WHY: 방 수용 한도(close 1013)나 협업 불가(1011)로 서버가 인증 전에 소켓을 닫으면 provider 4.6.0
 * 은 두 경로로 동시에 재접속한다. onOpen 이 재시도 루프의 취소 핸들을 지운 뒤라
 * (hocuspocus-provider.esm.js:189-193) onClose 의 setTimeout(connect) 가 새 루프를 만들고
 * (:404-414) 옛 루프도 자기 backoff 로 계속 돈다. 거절될 때마다 루프가 하나씩 늘어 간격이
 * 1 s → 20 ms 로 줄어드는 폭주가 된다. 이 거절만은 우리가 넘겨받는다: 그 소켓의 재접속을
 * 끄고(shouldConnect=false) 상한 있는 지수 backoff + jitter 뒤 소켓 층을 새로 세운다. */

import {
	HocuspocusProviderWebsocket,
	type HocuspocusProviderWebsocketConfiguration,
} from "@hocuspocus/provider";

export const RECONNECT_BASE_MS = 1_000;
export const RECONNECT_MAX_MS = 10_000;

/** Server close code for a capacity refusal ("try again later"). */
export const CLOSE_TRY_AGAIN_LATER = 1013;

export type CollabRefusal = "capacity" | "unavailable";

/** Equal-jitter exponential backoff: [ceiling/2, ceiling], ceiling = min(max, base·2^attempt). */
export function reconnectDelayMs(attempt: number, random: () => number = Math.random): number {
	const exponent = Math.max(0, Math.min(attempt, 16));
	const ceiling = Math.min(RECONNECT_MAX_MS, RECONNECT_BASE_MS * 2 ** exponent);
	const r = Math.min(Math.max(random(), 0), 1);
	return Math.round(ceiling / 2 + (ceiling / 2) * r);
}

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

/* WHY: destroy() 는 이미 걸린 onClose 의 setTimeout(connect) 를 지우지 못한다. 그 connect() 가
 * shouldConnect 를 다시 켜서 파기된 인스턴스가 리스너 없는 소켓을 연다(좀비). 파기 뒤 connect 는 무시한다. */
class OwnedSocket extends HocuspocusProviderWebsocket {
	private destroyed = false;

	connect(): Promise<unknown> {
		if (this.destroyed) return Promise.resolve();
		return super.connect();
	}

	destroy(): void {
		this.destroyed = true;
		super.destroy();
	}
}

/** A socket generation that hands refusals to `onRefused` instead of reconnecting.
 * `onClose` here runs before the provider's own close handler, so `disconnect()`
 * stops its reconnect timer and any pending retry loop aborts at its next attempt. */
export function createRefusalAwareSocket(
	configuration: HocuspocusProviderWebsocketConfiguration,
	onRefused: (refusal: CollabRefusal) => void,
): HocuspocusProviderWebsocket {
	const watch = new RefusalWatch();
	const socket: HocuspocusProviderWebsocket = new OwnedSocket({
		...configuration,
		onOpen: () => watch.open(),
		onMessage: () => watch.frame(),
		onClose: ({ event }) => {
			const refusal = watch.close(event?.code);
			if (refusal === null) return;
			socket.disconnect();
			onRefused(refusal);
		},
	});
	return socket;
}
