import assert from "node:assert/strict";
import test from "node:test";
import { HocuspocusProviderWebsocket } from "@hocuspocus/provider";
import {
	CLOSE_TRY_AGAIN_LATER,
	createRefusalAwareSocket,
	RECONNECT_BACKOFF,
	RefusalWatch,
	refusalOf,
} from "./collab-reconnect.ts";

test("열린 뒤 서버 프레임 없이 닫히면 거절이고 1013 만 수용 한도다", () => {
	assert.equal(refusalOf(true, CLOSE_TRY_AGAIN_LATER), "capacity");
	assert.equal(refusalOf(true, 1011), "unavailable");
	assert.equal(refusalOf(true, 1012), "unavailable");
	assert.equal(refusalOf(true, undefined), "unavailable");
	assert.equal(refusalOf(false, CLOSE_TRY_AGAIN_LATER), null);
});

test("RefusalWatch: 프레임을 받은 세션의 끊김과 열리지 않은 실패는 거절이 아니다", () => {
	const watch = new RefusalWatch();
	assert.equal(watch.close(1006), null, "never opened: provider backoff owns it");
	watch.open();
	watch.frame();
	assert.equal(watch.close(1013), null, "session had frames: normal reconnect");
	watch.open();
	assert.equal(watch.close(1013), "capacity");
	assert.equal(watch.close(1013), null, "reset after close");
});

let opened: number[] = [];

/** Opens, then the server closes before any frame — the room-cap refusal shape. */
class RefusingSocket extends EventTarget {
	readyState = 0;
	binaryType = "blob";
	readonly url: string;
	constructor(url: string) {
		super();
		this.url = url;
		setTimeout(() => {
			this.readyState = 1;
			opened.push(performance.now());
			this.dispatchEvent(new Event("open"));
			setTimeout(() => this.serverClose(), 1);
		}, 1);
	}
	protected serverClose() {
		if (this.readyState === 3) return;
		this.readyState = 3;
		this.dispatchEvent(Object.assign(new Event("close"), { code: 1013, reason: "try again later" }));
	}
	send() {}
	close() {
		this.readyState = 3;
	}
}

/** A served session: one server frame, then the connection drops (not a refusal). */
class DroppingSocket extends RefusingSocket {
	protected serverClose() {
		if (this.readyState === 3) return;
		this.dispatchEvent(Object.assign(new Event("message"), { data: new Uint8Array([0, 0]).buffer }));
		this.readyState = 3;
		this.dispatchEvent(Object.assign(new Event("close"), { code: 1001, reason: "going away" }));
	}
}

const FAST = {
	url: "ws://127.0.0.1:9/collab",
	delay: 20,
	minDelay: 20,
	maxDelay: 200,
};

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Opens while live for `liveMs`, then opens in the second after destroy(). */
async function measure(
	liveMs: number,
	make: () => { destroy(): void },
): Promise<{ live: number[]; afterDestroy: number }> {
	await sleep(1_000);
	opened = [];
	const socket = make();
	await sleep(liveMs);
	const live = opened;
	opened = [];
	socket.destroy();
	await sleep(1_000);
	return { live, afterDestroy: opened.length };
}

test("backoff 설정: 첫 재시도부터 jitter, 상한 있음, attempt 검증을 통과한다", () => {
	const { delay, minDelay, maxDelay, factor, jitter } = RECONNECT_BACKOFF;
	for (const value of [delay, minDelay, maxDelay]) assert.ok(Number.isInteger(value) && value > 0);
	assert.ok(minDelay < delay, "minDelay == delay would give every client the same first retry");
	assert.ok(delay <= maxDelay);
	assert.ok(factor > 1);
	assert.equal(jitter, true);
	const socket = createRefusalAwareSocket(
		{ url: "ws://127.0.0.1:9/collab", autoConnect: false, WebSocketPolyfill: RefusingSocket },
		() => {},
	);
	try {
		for (const key of ["delay", "minDelay", "maxDelay", "factor", "jitter"] as const) {
			assert.equal(socket.configuration[key], RECONNECT_BACKOFF[key], key);
		}
	} finally {
		socket.destroy();
	}
});

test("재현: provider 4.6.0 은 인증 전 거절마다 재시도 루프가 늘어 폭주한다", async () => {
	const { live } = await measure(
		1_500,
		() => new HocuspocusProviderWebsocket({ ...FAST, WebSocketPolyfill: RefusingSocket }),
	);
	/* 루프 하나라면 backoff(20→200 ms 상한)로 1.5 s 에 많아야 ~20 번이다. */
	assert.ok(live.length > 60, `expected a reconnect storm, got ${live.length} opens`);
	const gaps = live.slice(1).map((at, i) => at - live[i]);
	const lateGaps = gaps.slice(-20).sort((a, b) => a - b);
	assert.ok(lateGaps[10] < 20, `late gaps shrink below the base delay: ${lateGaps[10]} ms`);
});

test("재현: provider 4.6.0 은 파기 전에 예약된 재접속으로 파기 뒤에도 소켓을 연다", async () => {
	const { live, afterDestroy } = await measure(
		300,
		() =>
			new HocuspocusProviderWebsocket({ ...FAST, delay: 400, WebSocketPolyfill: DroppingSocket }),
	);
	assert.equal(live.length, 1);
	assert.equal(afterDestroy, 1, "the 400 ms reconnect timer fires on the destroyed instance");
});

/* Wider than FAST so a single loop (gaps ≥ minDelay) and a storm (gaps shrinking under it) differ. */
const REFUSE_FAST = {
	url: "ws://127.0.0.1:9/collab",
	delay: 60,
	minDelay: 30,
	maxDelay: 120,
};

test("거절된 소켓은 새 소켓 없이 한 루프의 backoff 로 다시 열고, 파기 뒤에는 열지 않는다", async () => {
	const refusals: string[] = [];
	const { live, afterDestroy } = await measure(1_500, () =>
		createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: RefusingSocket }, (refusal) =>
			refusals.push(refusal),
		),
	);
	/* 루프 하나: 매 간격이 minDelay 이상이라 1.5 s 에 많아야 50 번. 폭주는 수백 번이다. */
	assert.ok(live.length >= 4, `the socket retries by itself: ${live.length} opens`);
	assert.ok(live.length <= 1_500 / REFUSE_FAST.minDelay, `one bounded loop: ${live.length} opens`);
	const gaps = live.slice(1).map((at, i) => at - live[i]);
	const minGap = Math.min(...gaps);
	assert.ok(minGap >= REFUSE_FAST.minDelay - 3, `a gap below minDelay means a second loop: ${minGap} ms`);
	assert.deepEqual([...new Set(refusals)], ["capacity"]);
	assert.ok(refusals.length >= live.length - 1, `every open was refused: ${refusals.length}/${live.length}`);
	assert.equal(afterDestroy, 0, "destroy stops the retry loop and the reconnect timer");
});

test("서비스 중 끊긴 세션은 provider 가 다시 붙고, 파기하면 예약된 재접속도 열지 않는다", async () => {
	const refusals: string[] = [];
	const { live, afterDestroy } = await measure(300, () =>
		createRefusalAwareSocket(
			{ ...FAST, delay: 400, WebSocketPolyfill: DroppingSocket },
			(refusal) => refusals.push(refusal),
		),
	);
	assert.ok(live.length >= 1);
	assert.deepEqual(refusals, [], "a dropped served session is not a refusal");
	assert.equal(afterDestroy, 0, "the pending 400 ms reconnect must not fire after destroy");
});
