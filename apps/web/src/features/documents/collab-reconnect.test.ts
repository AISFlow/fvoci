import assert from "node:assert/strict";
import test from "node:test";
import { HocuspocusProviderWebsocket } from "@hocuspocus/provider";
import {
  CLOSE_TRY_AGAIN_LATER,
  type CollabRefusal,
  createRefusalAwareSocket,
  RECONNECT_BACKOFF,
  RefusalWatch,
  refusalOf,
} from "./collab-reconnect.ts";

await test("열린 뒤 서버 프레임 없이 닫히면 거절이고 1013 만 수용 한도다", () => {
  assert.equal(refusalOf(true, CLOSE_TRY_AGAIN_LATER), "capacity");
  assert.equal(refusalOf(true, 1011), "unavailable");
  assert.equal(refusalOf(true, 1012), "unavailable");
  assert.equal(refusalOf(true, undefined), "unavailable");
  assert.equal(refusalOf(false, CLOSE_TRY_AGAIN_LATER), null);
});

await test("RefusalWatch: 프레임을 받은 세션의 끊김과 열리지 않은 실패는 거절이 아니다", () => {
  const watch = new RefusalWatch();
  assert.equal(watch.close(1006), null, "never opened: provider backoff owns it");
  watch.open();
  watch.frame();
  assert.equal(watch.close(1013), null, "session had frames: normal reconnect");
  watch.open();
  assert.equal(watch.close(1013), "capacity");
  assert.equal(watch.close(1013), null, "reset after close");
});

await test("RefusalWatch: 거절 뒤 열리지도 못한 시도(서버 다운·오프라인)는 null 이라 기록된 거절을 지운다", () => {
  const watch = new RefusalWatch();
  watch.open();
  assert.equal(watch.close(CLOSE_TRY_AGAIN_LATER), "capacity");
  assert.equal(watch.close(1006), null, "error before open: not a refusal");
  assert.equal(watch.close(1006), null);
  watch.open();
  assert.equal(
    watch.close(CLOSE_TRY_AGAIN_LATER),
    "capacity",
    "refused again once the server is back",
  );
});

let opened: number[] = [];
/** Every socket constructed, in order: a reconnect is a new one. */
let made: RefusingSocket[] = [];
/** Runs right after a socket is constructed, before its open (tests that time the race). */
let onMade: ((socket: RefusingSocket) => void) | null = null;

/** Opens, then the server closes before any frame — the room-cap refusal shape. */
class RefusingSocket extends EventTarget {
  readyState = 0;
  binaryType = "blob";
  readonly url: string;
  constructor(url: string) {
    super();
    this.url = url;
    made.push(this);
    setTimeout(() => {
      /* Like a browser socket: one closed while connecting never opens. */
      if (this.readyState !== 0) return;
      this.readyState = 1;
      opened.push(performance.now());
      this.dispatchEvent(new Event("open"));
      setTimeout(() => {
        this.serverClose();
      }, 1);
    }, 1);
    onMade?.(this);
  }
  protected serverClose() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.dispatchEvent(
      Object.assign(new Event("close"), { code: 1013, reason: "try again later" }),
    );
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
    this.dispatchEvent(
      Object.assign(new Event("message"), { data: new Uint8Array([0, 0]).buffer }),
    );
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

async function until(condition: () => boolean, deadlineMs: number): Promise<void> {
  const end = performance.now() + deadlineMs;
  while (!condition() && performance.now() < end) await sleep(5);
}

/** Opens while live for `liveMs`; then what destroy() left: sockets it did not close, and the
 * sockets constructed and opens in the second after it. */
async function measure(
  liveMs: number,
  make: () => { destroy(): void },
): Promise<{ live: number[]; leftOpen: number; madeAfterDestroy: number; afterDestroy: number }> {
  await sleep(1_000);
  opened = [];
  made = [];
  const socket = make();
  await sleep(liveMs);
  const live = opened;
  opened = [];
  const before = made.length;
  socket.destroy();
  const leftOpen = made.filter((ws) => ws.readyState !== 3).length;
  await sleep(1_000);
  return { live, leftOpen, madeAfterDestroy: made.length - before, afterDestroy: opened.length };
}

await test("backoff 설정: 첫 재시도부터 jitter, 상한 있음, attempt 검증을 통과한다", () => {
  const policy: {
    delay: number;
    minDelay: number;
    maxDelay: number;
    factor: number;
    jitter: boolean;
  } = RECONNECT_BACKOFF;
  const { delay, minDelay, maxDelay, factor, jitter } = policy;
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

await test("재현: provider 4.6.0 은 인증 전 거절마다 재시도 루프가 늘어 폭주한다", async () => {
  const { live } = await measure(
    1_500,
    () => new HocuspocusProviderWebsocket({ ...FAST, WebSocketPolyfill: RefusingSocket }),
  );
  /* 루프 하나라면 backoff(20→200 ms 상한)로 1.5 s 에 많아야 ~20 번이다. */
  assert.ok(live.length > 60, `expected a reconnect storm, got ${String(live.length)} opens`);
  const gaps = live.slice(1).map((at, i) => at - live[i]);
  const lateGaps = gaps.slice(-20).sort((a, b) => a - b);
  assert.ok(lateGaps[10] < 20, `late gaps shrink below the base delay: ${String(lateGaps[10])} ms`);
});

await test("재현: provider 4.6.0 은 파기 전에 예약된 재접속으로 파기 뒤에도 소켓을 연다", async () => {
  const { live, madeAfterDestroy, afterDestroy } = await measure(
    300,
    () =>
      new HocuspocusProviderWebsocket({ ...FAST, delay: 400, WebSocketPolyfill: DroppingSocket }),
  );
  assert.equal(live.length, 1);
  assert.equal(
    madeAfterDestroy,
    1,
    "the 400 ms reconnect timer builds a socket on the destroyed instance",
  );
  assert.equal(afterDestroy, 1, "and nothing closes it before it opens");
});

/* Wider than FAST so a single loop (gaps ≥ minDelay) and a storm (gaps shrinking under it) differ. */
const REFUSE_FAST = {
  url: "ws://127.0.0.1:9/collab",
  delay: 60,
  minDelay: 30,
  maxDelay: 120,
};

await test("거절된 소켓은 새 소켓 없이 한 루프의 backoff 로 다시 열고, 파기 뒤에는 열지 않는다", async () => {
  const refusals: Array<CollabRefusal | null> = [];
  const { live, leftOpen, madeAfterDestroy, afterDestroy } = await measure(1_500, () =>
    createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: RefusingSocket }, (refusal) =>
      refusals.push(refusal),
    ),
  );
  /* 루프 하나: 매 간격이 minDelay 이상이라 1.5 s 에 많아야 50 번. 폭주는 수백 번이다. */
  assert.ok(live.length >= 4, `the socket retries by itself: ${String(live.length)} opens`);
  assert.ok(
    live.length <= 1_500 / REFUSE_FAST.minDelay,
    `one bounded loop: ${String(live.length)} opens`,
  );
  const gaps = live.slice(1).map((at, i) => at - live[i]);
  const minGap = Math.min(...gaps);
  assert.ok(
    minGap >= REFUSE_FAST.minDelay - 3,
    `a gap below minDelay means a second loop: ${String(minGap)} ms`,
  );
  assert.deepEqual([...new Set(refusals)], ["capacity"]);
  assert.ok(
    refusals.length >= live.length - 1,
    `every open was refused: ${String(refusals.length)}/${String(live.length)}`,
  );
  assert.equal(leftOpen, 0, "destroy closes the socket it had, connecting or open");
  assert.equal(madeAfterDestroy, 0, "destroy stops the retry loop and the reconnect timer");
  assert.equal(afterDestroy, 0);
});

await test("서비스 중 끊긴 세션은 provider 가 다시 붙고, 파기하면 예약된 재접속도 열지 않는다", async () => {
  const refusals: Array<CollabRefusal | null> = [];
  const { live, leftOpen, madeAfterDestroy, afterDestroy } = await measure(300, () =>
    createRefusalAwareSocket(
      { ...FAST, delay: 400, WebSocketPolyfill: DroppingSocket },
      (refusal) => refusals.push(refusal),
    ),
  );
  assert.ok(live.length >= 1);
  assert.ok(refusals.length >= 1, "the drop is reported so a stale refusal would be cleared");
  assert.ok(
    refusals.every((refusal) => refusal === null),
    `a dropped served session is not a refusal: ${String(refusals)}`,
  );
  assert.equal(leftOpen, 0);
  assert.equal(madeAfterDestroy, 0, "the pending 400 ms reconnect must not fire after destroy");
  assert.equal(afterDestroy, 0);
});

/* The race behind a CI flake of the refused-socket test above (afterDestroy 1 !== 0): the retry
 * timer constructs a socket and destroy() runs in the same timer pass, before that socket opens.
 * The old fake opened it anyway although destroy() had closed it; a browser socket does not. */
await test("파기 직전에 만든 연결 중 소켓은 파기가 닫아 열리지 않고, 파기 뒤 새 소켓은 없다", async () => {
  opened = [];
  made = [];
  let atDestroy = null as { state: number; closed: number; made: number } | null;
  const socket = createRefusalAwareSocket(
    { ...REFUSE_FAST, WebSocketPolyfill: RefusingSocket },
    () => {},
  );
  onMade = (ws) => {
    if (made.length < 3) return;
    onMade = null;
    queueMicrotask(() => {
      const state = ws.readyState;
      opened = [];
      socket.destroy();
      atDestroy = { state, closed: ws.readyState, made: made.length };
    });
  };
  try {
    await until(() => atDestroy !== null, 5_000);
    assert.ok(atDestroy, "a third socket was constructed");
    assert.equal(atDestroy.state, 0, "destroy ran while that socket was still connecting");
    assert.equal(atDestroy.closed, 3, "destroy closes it");
    /* Longer than any reconnect the socket had pending: onClose's delay, the loop's backoff. */
    await sleep(REFUSE_FAST.delay + REFUSE_FAST.maxDelay + 50);
    assert.equal(made.length, atDestroy.made, "no socket after destroy");
    assert.equal(opened.length, 0, "a socket closed while connecting never opens");
    assert.ok(
      made.every((ws) => ws.readyState === 3),
      "every socket ends closed",
    );
  } finally {
    onMade = null;
    socket.destroy();
  }
});
