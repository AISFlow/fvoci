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
  /** Reports one close of this socket: its refusal, or null for a close that was not one. */
  readonly refuse: (refusal: CollabRefusal | null) => void;
  private readonly log: string[];
  retired = 0;
  destroyed = 0;

  constructor(id: number, refuse: (refusal: CollabRefusal | null) => void, log: string[]) {
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

  retire(): void {
    this.retired += 1;
    this.log.push(`retire#${this.id}`);
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
    open: (onClosed) => {
      const socket = new FakeSocket(sockets.length, onClosed, log);
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
  assert.equal(
    timers.length,
    0,
    "the socket's own backoff retries; the controller schedules nothing",
  );
  assert.equal(sockets[0].retired, 0, "a refusal does not stop the socket's retries");
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

test("거절 뒤 거절이 아닌 close(열리지도 못한 시도)는 사유를 지우고, 다시 거절되면 다시 기록한다", () => {
  const { room, sockets, states } = harness();
  sockets[0].refuse("capacity");
  sockets[0].refuse(null);
  assert.equal(room.state.refusal, null, "the status falls back to the raw connection state");
  sockets[0].refuse(null);
  assert.equal(states.length, 2, "repeated non-refusal closes change nothing");
  sockets[0].refuse("capacity");
  assert.deepEqual(
    states.map((s) => s.refusal),
    ["capacity", null, "capacity"],
  );
  assert.equal(room.state.socket, sockets[0]);
  assert.equal(room.state.generation, 0);
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

test("재선언만 소켓 세대를 바꾼다: clientID 교체 뒤 새 소켓, 옛 소켓은 재접속을 멈추고 flush 뒤 지연 파기", () => {
  const { room, log, sockets, timers, runTimers } = harness();
  sockets[0].attach("w:document:d");
  sockets[0].refuse("capacity");

  assert.equal(room.reclaim(), true);
  assert.deepEqual(log, ["open#0", "swap-client-id", "retire#0", "flush#0", "open#1"]);
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

test("dispose: 재접속부터 멈추고, 열린 소켓으로 밀린 편집을 먼저 보낸 뒤 파기하고, 뒤늦은 사건은 무시한다", () => {
  const { room, log, sockets, states, timers, runTimers } = harness();
  sockets[0].attach("w:document:d");
  room.dispose();
  assert.deepEqual(
    log,
    ["open#0", "retire#0", "flush#0"],
    "no reconnect from here on; the flush runs while the socket is still open",
  );
  assert.equal(sockets[0].destroyed, 0);
  assert.equal(timers.length, 1);
  runTimers();
  assert.deepEqual(log, ["open#0", "retire#0", "flush#0", "destroy#0"]);

  const before = states.length;
  sockets[0].refuse("capacity");
  room.authenticated();
  assert.equal(room.reclaim(), false);
  room.dispose();
  runTimers();
  assert.equal(states.length, before, "no state after dispose");
  assert.equal(sockets.length, 1, "no socket after dispose");
  assert.equal(sockets[0].retired, 1, "dispose is idempotent");
  assert.equal(sockets[0].destroyed, 1, "dispose is idempotent");
});

/* A server that refuses planned opens (close 1013 after reading auth, no frame first — the room
 * cap shape), serves others (an Authenticated frame), and is down for "down" attempts: the
 * browser fires error then close 1006 and the socket never opens (server stopped, offline, 503). */
let plan: Array<"refuse" | "serve" | "down"> = [];
/** Every socket constructed, in order: a reconnect is a new one. */
let servers: PlannedServer[] = [];
/** Runs right after a socket is constructed, before it opens (tests that time the race). */
let onMade: ((socket: PlannedServer) => void) | null = null;
let attempts: number[] = [];
let opens: number[] = [];
let downs = 0;
let authFrames = 0;
let served: PlannedServer | null = null;

class PlannedServer extends EventTarget {
  readyState = 0;
  binaryType = "blob";
  readonly url: string;
  private readonly mode: "refuse" | "serve" | "down";

  constructor(url: string) {
    super();
    this.url = url;
    this.mode = plan.shift() ?? "refuse";
    servers.push(this);
    attempts.push(performance.now());
    setTimeout(() => {
      if (this.readyState !== 0) return;
      if (this.mode === "down") {
        downs += 1;
        this.readyState = 3;
        this.dispatchEvent(new Event("error"));
        this.dispatchEvent(Object.assign(new Event("close"), { code: 1006, reason: "" }));
        return;
      }
      this.readyState = 1;
      opens.push(performance.now());
      this.dispatchEvent(new Event("open"));
    }, 1);
    onMade?.(this);
  }

  /** Message types this socket delivered to the server, in order. */
  readonly received: number[] = [];

  send(data: Uint8Array) {
    if (this.readyState !== 1) return;
    const decoder = decoding.createDecoder(new Uint8Array(data));
    const name = decoding.readVarString(decoder);
    const type = decoding.readVarUint(decoder);
    this.received.push(type);
    if (type !== MessageType.Auth) return;
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
  servers = [];
  attempts = [];
  opens = [];
  downs = 0;
  authFrames = 0;
  served = null;
  const refusals: Array<CollabRefusal | null> = [];
  let authenticated = 0;
  const room = new RoomConnection({
    open: (onClosed) =>
      createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: PlannedServer }, onClosed),
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
    assert.ok(
      again.length <= 1_000 / REFUSE_FAST.minDelay,
      `one bounded loop: ${again.length} opens`,
    );
    assert.ok(
      gapsOf(again).every((gap) => gap >= REFUSE_FAST.minDelay - 3),
      `a gap below minDelay means a second loop: ${gapsOf(again).map(Math.round)}`,
    );
    assert.equal(room.state.refusal, "capacity");
    assert.equal(room.state.socket, socket);
    assert.equal(authFrames, opens.length);

    const madeAtDispose = servers.length;
    const openAtDispose = opens.length;
    /* The socket may be connecting: dispose destroys it one task later, and it can open first. */
    const connectingAtDispose = servers.filter((ws) => ws.readyState === 0).length;
    tearDown();
    await sleep(0);
    assert.ok(
      servers.every((ws) => ws.readyState === 3),
      "one task after dispose the socket is closed, whether it was open or connecting",
    );
    await sleep(600);
    assert.equal(servers.length, madeAtDispose, "dispose stops the retry loop and reconnect timer");
    const lateOpens = opens.length - openAtDispose;
    assert.ok(
      lateOpens <= connectingAtDispose,
      `only a socket connecting at dispose may still open: ${lateOpens} opens, ${connectingAtDispose} connecting`,
    );
    assert.ok(
      servers.every((ws) => ws.readyState === 3),
      "every socket ends closed",
    );
  } finally {
    /* A failed assertion must not leave the socket retrying and the test process alive. */
    if (!tornDown) tearDown();
  }
});

test("실제 provider: 거절 뒤 서버가 내려가 열리지도 못하는 동안은 거절로 표시하지 않고, 다시 거절되면 다시 기록한다", async () => {
  plan = ["refuse", "down", "down", "down", "refuse", "serve"];
  servers = [];
  attempts = [];
  opens = [];
  downs = 0;
  authFrames = 0;
  served = null;
  const closes: Array<CollabRefusal | null> = [];
  const refusals: Array<CollabRefusal | null> = [];
  let authenticated = 0;
  const room = new RoomConnection({
    open: (onClosed) =>
      createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: PlannedServer }, (refusal) => {
        closes.push(refusal);
        onClosed(refusal);
      }),
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
    assert.equal(downs, 3);
    assert.equal(attempts.length, 6, "refused, three attempts that never opened, refused, served");
    assert.equal(opens.length, 3, "the down attempts never opened");
    assert.deepEqual(
      closes,
      ["capacity", null, null, null, "capacity"],
      "every close is reported; one that never opened is not a refusal",
    );
    assert.deepEqual(
      refusals,
      ["capacity", null, "capacity", null],
      "busy, then the raw connection state while the server is down, busy again, cleared on authenticate",
    );
    assert.equal(room.state.socket, socket, "one socket for the whole episode");
    assert.equal(room.state.generation, 0);
    assert.ok(
      gapsOf(attempts).every((gap) => gap >= REFUSE_FAST.minDelay - 3),
      `the outage stays on one bounded loop: ${gapsOf(attempts).map(Math.round)}`,
    );
  } finally {
    if (!tornDown) tearDown();
  }
});

function resetServer(next: typeof plan): void {
  plan = next;
  onMade = null;
  servers = [];
  attempts = [];
  opens = [];
  downs = 0;
  authFrames = 0;
  served = null;
}

/** A room as the hosts run it: the provider is destroyed after the socket. With `hold`, the
 * deferred socket destroy runs only when the test says — the gap a busy or throttled tab leaves
 * between dispose and that task. */
function plannedRoom({ hold, onRefused }: { hold: boolean; onRefused?: () => void }) {
  const held: Array<() => void> = [];
  let authenticated = 0;
  const room = new RoomConnection({
    open: (onClosed) =>
      createRefusalAwareSocket({ ...REFUSE_FAST, WebSocketPolyfill: PlannedServer }, (refusal) => {
        onClosed(refusal);
        if (refusal === "capacity") onRefused?.();
      }),
    onChange: () => {},
    beforeReclaim: () => assert.fail("no authenticationFailed in this scenario"),
    reclaimLimit: 3,
    timers: {
      setTimeout: (callback, ms) => {
        if (hold) held.push(callback);
        else setTimeout(callback, ms);
      },
    },
  });
  const doc = new Y.Doc();
  const provider = new HocuspocusProvider({
    websocketProvider: room.state.socket,
    name: "w:document:d",
    document: doc,
    token: "1",
    flushDelay: 200,
    onAuthenticated: () => {
      authenticated += 1;
      room.authenticated();
    },
  });
  provider.attach();
  const runHeld = () => {
    for (const run of held.splice(0)) run();
  };
  const tearDown = () => {
    onMade = null;
    room.dispose();
    runHeld();
    provider.destroy();
  };
  return { room, doc, held, runHeld, tearDown, authenticated: () => authenticated };
}

/* The race behind a CI flake of the first real-provider test (opens 18 !== 17): the retry timer
 * constructs a socket and dispose runs in the same timer pass. That socket can open before the
 * deferred destroy closes it; it is not a socket made after dispose. */
test("실제 provider: dispose 직전에 만든 연결 중 소켓은 한 task 안에 닫히고, dispose 뒤 새 소켓은 없다", async () => {
  resetServer([]);
  const { room, tearDown } = plannedRoom({ hold: false });
  let atDispose = null as { made: number; opens: number; state: number } | null;
  onMade = (socket) => {
    if (servers.length < 3) return;
    onMade = null;
    queueMicrotask(() => {
      atDispose = { made: servers.length, opens: opens.length, state: socket.readyState };
      room.dispose();
    });
  };
  try {
    await until(() => atDispose !== null, 5_000);
    assert.ok(atDispose, "a third socket was constructed");
    assert.equal(atDispose.state, 0, "dispose ran while that socket was still connecting");
    await sleep(0);
    assert.ok(
      servers.every((ws) => ws.readyState === 3),
      "one task after dispose that socket is closed",
    );
    assert.ok(
      opens.length - atDispose.opens <= 1,
      "only that socket may have opened, before the destroy",
    );
    await sleep(REFUSE_FAST.delay + REFUSE_FAST.maxDelay);
    assert.equal(servers.length, atDispose.made, "no socket after dispose");
    assert.ok(
      servers.every((ws) => ws.readyState === 3),
      "every socket ends closed",
    );
  } finally {
    tearDown();
  }
});

test("실제 provider: dispose 는 소켓 파기 전에 재접속부터 멈춘다 — 파기가 늦어도 dispose 뒤 새 소켓은 없다", async () => {
  resetServer([]);
  let refused = 0;
  let madeAtDispose = -1;
  const { room, held, runHeld, tearDown } = plannedRoom({
    hold: true,
    onRefused: () => {
      refused += 1;
      if (refused !== 2) return;
      /* Right after a refusal: the socket's loop is waiting out its backoff. */
      queueMicrotask(() => {
        madeAtDispose = servers.length;
        room.dispose();
      });
    },
  });
  try {
    await until(() => madeAtDispose >= 0, 5_000);
    assert.ok(madeAtDispose >= 2, "disposed after the second refusal");
    assert.equal(held.length, 1, "the socket's destroy is deferred");
    /* Several backoffs and refused attempts long: the loop would have opened a socket by now. */
    await sleep(3 * REFUSE_FAST.maxDelay);
    assert.equal(servers.length, madeAtDispose, "no socket while the destroy is pending");
    runHeld();
    await sleep(REFUSE_FAST.delay + REFUSE_FAST.maxDelay);
    assert.equal(servers.length, madeAtDispose, "no socket after the destroy");
    assert.ok(
      servers.every((ws) => ws.readyState === 3),
      "every socket ends closed",
    );
  } finally {
    tearDown();
  }
});

test("실제 provider: dispose 는 열린 소켓을 닫지 않고 밀린 편집을 그 소켓으로 보낸 뒤, 지연 파기가 닫는다", async () => {
  resetServer(["serve"]);
  const { room, doc, held, runHeld, tearDown, authenticated } = plannedRoom({ hold: true });
  try {
    await until(() => authenticated() > 0, 5_000);
    assert.ok(served, "authenticated");
    const socket = served as PlannedServer;
    const delivered = socket.received.length;
    doc.getText("t").insert(0, "x");
    assert.equal(socket.received.length, delivered, "the edit waits for the 200 ms batch");
    room.dispose();
    assert.deepEqual(
      socket.received.slice(delivered),
      [MessageType.Sync],
      "dispose flushes the batched edit on the socket",
    );
    assert.equal(socket.readyState, 1, "and leaves it open");
    await sleep(REFUSE_FAST.maxDelay);
    assert.equal(socket.readyState, 1, "until the deferred destroy");
    assert.equal(held.length, 1);
    runHeld();
    assert.equal(socket.readyState, 3, "the deferred destroy closes it");
    await sleep(REFUSE_FAST.delay + REFUSE_FAST.maxDelay);
    assert.equal(servers.length, 1, "one socket, no reconnect");
  } finally {
    tearDown();
  }
});
