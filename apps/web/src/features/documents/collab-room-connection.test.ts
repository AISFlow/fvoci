import assert from "node:assert/strict";
import test from "node:test";
import { HocuspocusProvider, MessageType } from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as Y from "yjs";
import {
	type CollabRefusal,
	createRefusalAwareSocket,
	RoomConnection,
	type RoomConnectionState,
	type RoomSocketHandle,
} from "./collab-reconnect.ts";

type Flushable = { flushPendingUpdates(): void };

class FakeSocket implements RoomSocketHandle {
	readonly configuration = { providerMap: new Map<string, Flushable>() };
	readonly id: number;
	readonly refuse: (refusal: CollabRefusal) => void;
	private readonly log: string[];
	destroyed = 0;

	constructor(id: number, refuse: (refusal: CollabRefusal) => void, log: string[]) {
		this.id = id;
		this.refuse = refuse;
		this.log = log;
	}

	/** A room provider attached to this socket; its flush is logged in order. */
	attach(name: string): void {
		this.configuration.providerMap.set(name, {
			flushPendingUpdates: () => this.log.push(`flush#${this.id}`),
		});
	}

	destroy(): void {
		this.destroyed += 1;
		this.log.push(`destroy#${this.id}`);
	}
}

function harness(reclaimLimit = 3) {
	const log: string[] = [];
	const sockets: FakeSocket[] = [];
	const states: RoomConnectionState<FakeSocket>[] = [];
	const timers: Array<() => void> = [];
	const room = new RoomConnection<FakeSocket>({
		open: (onRefused) => {
			const socket = new FakeSocket(sockets.length, onRefused, log);
			sockets.push(socket);
			log.push(`open#${socket.id}`);
			return socket;
		},
		onChange: (state) => states.push(state),
		beforeReclaim: () => log.push("swap-client-id"),
		reclaimLimit,
		timers: {
			setTimeout: (callback, ms) => {
				assert.equal(ms, 0, "teardown is deferred by one task only");
				timers.push(callback);
				return timers.length;
			},
		},
	});
	const runTimers = () => {
		for (const callback of timers.splice(0)) callback();
	};
	return { room, log, sockets, states, timers, runTimers };
}

test("거절은 사유만 기록한다: 새 소켓·세대·타이머 없이 같은 소켓이 스스로 다시 시도한다", () => {
	const { room, sockets, states, timers } = harness();
	assert.equal(sockets.length, 1);
	assert.equal(room.state.socket, sockets[0]);
	assert.equal(room.state.generation, 0);
	assert.equal(room.state.refusal, null);

	sockets[0].refuse("capacity");
	assert.equal(room.state.refusal, "capacity");
	assert.equal(room.state.socket, sockets[0], "no new socket per refusal");
	assert.equal(room.state.generation, 0, "consumers are not remounted by a refusal");
	assert.equal(sockets.length, 1);
	assert.equal(timers.length, 0, "the socket's own backoff retries; the controller schedules nothing");
	assert.equal(sockets[0].destroyed, 0);
	assert.deepEqual(
		states.map((s) => s.refusal),
		["capacity"],
	);

	sockets[0].refuse("capacity");
	assert.equal(states.length, 1, "the same refusal again changes nothing");
	sockets[0].refuse("unavailable");
	assert.equal(room.state.refusal, "unavailable");
	assert.equal(states.length, 2);
});

test("인증되면 거절 사유를 지우고 재선언 예산을 되채운다", () => {
	const { room, sockets, states } = harness(2);
	sockets[0].refuse("capacity");
	room.authenticated();
	assert.equal(room.state.refusal, null);
	assert.equal(states.length, 2);
	room.authenticated();
	assert.equal(states.length, 2, "authenticating without a refusal emits nothing");

	assert.equal(room.reclaim(), true);
	assert.equal(room.reclaim(), true);
	assert.equal(room.reclaim(), false, "budget of 2 is spent");
	assert.equal(sockets.length, 3);
	room.authenticated();
	assert.equal(room.reclaim(), true, "an authentication refills the budget");
	assert.equal(sockets.length, 4);
});

test("재선언만 소켓 세대를 바꾼다: clientID 교체 뒤 새 소켓, 옛 소켓은 flush 뒤 지연 파기", () => {
	const { room, log, sockets, timers, runTimers } = harness();
	sockets[0].attach("w:document:d");
	sockets[0].refuse("capacity");

	assert.equal(room.reclaim(), true);
	assert.deepEqual(log, ["open#0", "swap-client-id", "flush#0", "open#1"]);
	assert.equal(room.state.socket, sockets[1]);
	assert.equal(room.state.generation, 1);
	assert.equal(room.state.refusal, null, "the old socket's refusal does not carry over");
	assert.equal(sockets[0].destroyed, 0, "destroy waits one task so the room can detach");
	assert.equal(timers.length, 1);
	runTimers();
	assert.equal(sockets[0].destroyed, 1);

	sockets[0].refuse("capacity");
	assert.equal(room.state.refusal, null, "a late refusal of a replaced socket is ignored");
	sockets[1].refuse("unavailable");
	assert.equal(room.state.refusal, "unavailable");
});

test("dispose: 열린 소켓으로 밀린 편집을 먼저 보낸 뒤 파기하고, 뒤늦은 사건은 무시한다", () => {
	const { room, log, sockets, states, timers, runTimers } = harness();
	sockets[0].attach("w:document:d");
	room.dispose();
	assert.deepEqual(log, ["open#0", "flush#0"], "flush runs while the socket is still open");
	assert.equal(sockets[0].destroyed, 0);
	assert.equal(timers.length, 1);
	runTimers();
	assert.deepEqual(log, ["open#0", "flush#0", "destroy#0"]);

	const before = states.length;
	sockets[0].refuse("capacity");
	room.authenticated();
	assert.equal(room.reclaim(), false);
	room.dispose();
	runTimers();
	assert.equal(states.length, before, "no state after dispose");
	assert.equal(sockets.length, 1, "no socket after dispose");
	assert.equal(sockets[0].destroyed, 1, "dispose is idempotent");
});

/* A server that refuses planned opens (close 1013 after reading auth, no frame first — the room
 * cap shape) and serves the others (an Authenticated frame). */
let plan: Array<"refuse" | "serve"> = [];
let opens: number[] = [];
let authFrames = 0;
let served: PlannedServer | null = null;

class PlannedServer extends EventTarget {
	readyState = 0;
	binaryType = "blob";
	readonly url: string;
	private readonly mode: "refuse" | "serve";

	constructor(url: string) {
		super();
		this.url = url;
		this.mode = plan.shift() ?? "refuse";
		setTimeout(() => {
			if (this.readyState !== 0) return;
			this.readyState = 1;
			opens.push(performance.now());
			this.dispatchEvent(new Event("open"));
		}, 1);
	}

	send(data: Uint8Array) {
		if (this.readyState !== 1) return;
		const decoder = decoding.createDecoder(new Uint8Array(data));
		const name = decoding.readVarString(decoder);
		if (decoding.readVarUint(decoder) !== MessageType.Auth) return;
		authFrames += 1;
		setTimeout(() => {
			if (this.readyState !== 1) return;
			if (this.mode === "refuse") {
				this.readyState = 3;
				this.dispatchEvent(
					Object.assign(new Event("close"), { code: 1013, reason: "try again later" }),
				);
				return;
			}
			served = this;
			const encoder = encoding.createEncoder();
			encoding.writeVarString(encoder, name);
			encoding.writeVarUint(encoder, MessageType.Auth);
			encoding.writeVarUint(encoder, 2 /* Authenticated */);
			encoding.writeVarString(encoder, "read-write");
			const frame = encoding.toUint8Array(encoder);
			this.dispatchEvent(Object.assign(new Event("message"), { data: frame.buffer }));
		}, 1);
	}

	drop() {
		if (this.readyState === 3) return;
		this.readyState = 3;
		this.dispatchEvent(Object.assign(new Event("close"), { code: 1006, reason: "" }));
	}

	close() {
		this.readyState = 3;
	}
}

const REFUSE_FAST = {
	url: "ws://127.0.0.1:9/collab",
	delay: 60,
	minDelay: 30,
	maxDelay: 120,
};

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(condition: () => boolean, deadlineMs: number): Promise<void> {
	const end = performance.now() + deadlineMs;
	while (!condition() && performance.now() < end) await sleep(5);
}

function gapsOf(at: number[]): number[] {
	return at.slice(1).map((t, i) => t - at[i]);
}

test("실제 provider: 거절 세 번 뒤 같은 소켓으로 인증되고, 끊긴 뒤 다시 거절돼도 루프는 하나이며, dispose 뒤엔 열지 않는다", async () => {
	plan = ["refuse", "refuse", "refuse", "serve"];
	opens = [];
	authFrames = 0;
	served = null;
	const refusals: Array<CollabRefusal | null> = [];
	let authenticated = 0;
	const room = new RoomConnection({
		open: (onRefused) =>
			createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: PlannedServer }, onRefused),
		onChange: (state) => refusals.push(state.refusal),
		beforeReclaim: () => assert.fail("no authenticationFailed in this scenario"),
		reclaimLimit: 3,
		timers: { setTimeout: (callback, ms) => setTimeout(callback, ms) },
	});
	const socket = room.state.socket;
	const provider = new HocuspocusProvider({
		websocketProvider: socket,
		name: "w:document:d",
		document: new Y.Doc(),
		token: "1",
		onAuthenticated: () => {
			authenticated += 1;
			room.authenticated();
		},
	});
	provider.attach();
	let tornDown = false;
	const tearDown = () => {
		tornDown = true;
		room.dispose();
		provider.destroy();
	};
	try {
		await until(() => authenticated > 0, 5_000);
		assert.equal(authenticated, 1);
		assert.equal(opens.length, 4, "three refused opens, then the served one");
		assert.equal(authFrames, 4, "the provider re-sends auth on every open of the same socket");
		assert.deepEqual(refusals, ["capacity", null], "busy while refused, cleared on authenticate");
		assert.equal(room.state.socket, socket, "one socket for the whole episode");
		assert.equal(room.state.generation, 0);
		assert.ok(
			gapsOf(opens).every((gap) => gap >= REFUSE_FAST.minDelay - 3),
			`refusal retries wait at least minDelay: ${gapsOf(opens).map(Math.round)}`,
		);

		const settled = opens.length;
		await sleep(300);
		assert.equal(opens.length, settled, "a served session is left alone");

		/* The served session drops; every reconnect is refused from here on. */
		assert.ok(served);
		(served as PlannedServer).drop();
		const dropAt = opens.length;
		await sleep(1_000);
		const again = opens.slice(dropAt);
		assert.ok(again.length >= 3, `the provider keeps retrying: ${again.length} opens`);
		assert.ok(again.length <= 1_000 / REFUSE_FAST.minDelay, `one bounded loop: ${again.length} opens`);
		assert.ok(
			gapsOf(again).every((gap) => gap >= REFUSE_FAST.minDelay - 3),
			`a gap below minDelay means a second loop: ${gapsOf(again).map(Math.round)}`,
		);
		assert.equal(room.state.refusal, "capacity");
		assert.equal(room.state.socket, socket);
		assert.equal(authFrames, opens.length);

		tearDown();
		const disposedAt = opens.length;
		await sleep(600);
		assert.equal(opens.length, disposedAt, "dispose stops the retry loop and reconnect timer");
	} finally {
		/* A failed assertion must not leave the socket retrying and the test process alive. */
		if (!tornDown) tearDown();
	}
});
