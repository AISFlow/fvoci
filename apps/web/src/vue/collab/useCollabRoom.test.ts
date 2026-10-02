import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { EventEmitter } from "node:events";
import { runInNewContext } from "node:vm";
import * as Vue from "vue";
import * as Y from "yjs";
import ts from "typescript";
import * as model from "../../features/documents/collab-model";
import * as ack from "../../features/documents/collab-persist-ack";

// The React glue's source assertions (features/documents/collab-session.test.ts),
// ported to the Vue room composable. Like those, they read the source: the
// repo has no DOM test setup, and the room opens real sockets. The browser
// behaviour (one socket per room, teardown on navigation, reconnect, persist
// ACK) is covered by e2e/workspace-wiki-vue-flow.spec.ts and the collab suite.

const roomPath = path.join(import.meta.dirname, "useCollabRoom.ts");

function source(): string {
  return readFileSync(roomPath, "utf8").replace(/\/\*[\s\S]*?\*\/|\/\/.*/g, "");
}

function between(src: string, start: string, end: string): string {
  const from = src.indexOf(start);
  assert.notEqual(from, -1, `missing ${start}`);
  const to = src.indexOf(end, from);
  assert.notEqual(to, -1, `missing ${end} after ${start}`);
  return src.slice(from, to);
}

await test("useCollabRoom binds @hocuspocus/provider itself: one provider per socket generation", () => {
  const src = source();
  assert.equal(src.includes("@hocuspocus/provider-react"), false);
  assert.equal(src.match(/new HocuspocusProvider\(/g)?.length, 1);
  const bind = between(src, "function bindGeneration(", "function retire(");
  assert.match(bind, /websocketProvider: state\.socket,/);
  assert.match(bind, /token: String\(doc\.clientID\),/);
  assert.match(bind, /flushDelay: 200,/);
  assert.match(bind, /provider\.attach\(\);/);
  assert.equal(src.includes("gc: false"), true);
  assert.equal(src.includes("durableSaved"), true);
  assert.equal(src.includes("syncPersistBind"), true);
  assert.equal(src.includes("scopedPersistObserver"), true);
});

await test("useCollabRoom keeps Yjs and provider objects raw, never reactive", () => {
  const src = source();
  assert.equal(/\breactive\(/.test(src), false);
  assert.equal(/\bref\(/.test(src), false, "shallowRef only");
  assert.match(src, /const doc = markRaw\(new Y\.Doc\(\{ gc: false \}\)\);/);
  assert.match(
    src,
    /open: \(onClosed\) => markRaw\(createRefusalAwareSocket\(\{ url \}, onClosed\)\),/,
  );
  assert.match(src, /const provider = markRaw\(\s*new HocuspocusProvider\(/);
});

await test("useCollabRoom has no hex literals", () => {
  assert.equal(/#[0-9a-fA-F]{3,8}/.test(source()), false);
});

await test("useCollabRoom hands the room's auth results to the connection state machine", () => {
  const bind = between(source(), "function bindGeneration(", "function retire(");
  assert.match(bind, /const onAuthenticated = \(\) => \{\s*connection\.authenticated\(\);\s*\};/);
  assert.match(
    bind,
    /if \(!retiredAuthorization\.has\(provider\) && !reauthorizing\.delete\(provider\)\)\s*connection\.reclaim\(\);/,
  );
  assert.match(bind, /provider\.off\("authenticated", onAuthenticated\);/);
});

await test("useCollabRoom re-binds only on a new socket generation, never on a refusal", () => {
  const src = source();
  const watches = [...src.matchAll(/watch\(\s*\(\) => room\.value\.([a-zA-Z]+),/g)].map(
    (m) => m[1],
  );
  assert.deepEqual(
    watches,
    ["generation"],
    "a watch on anything a refusal changes re-binds per refusal",
  );
  assert.match(src, /reclaimLimit: CLAIM_RETRY_LIMIT,/);
  assert.match(src, /doc\.clientID = new Y\.Doc\(\)\.clientID;/);
});

await test("useCollabRoom decides the session status with collabStatusOf only", () => {
  const session = between(source(), "function bindSession(", "bindGeneration(connection.state);");
  assert.match(
    session,
    /status: collabStatusOf\(unauthorized\.value, room\.value\.refusal, connectionStatus\.value\),/,
  );
  assert.match(session, /pending: unsent\.value && !readOnly\.value,/);
  assert.match(session, /durableSaved: isDurablySaved\(bind\.value\.ack\),/);
});

await test("useCollabRoom tears down: flush and socket first, then the provider after 0 ms", () => {
  const src = source();
  const dispose = between(src, "onScopeDispose(() => {\n    disposed = true;", "return {");
  const flush = dispose.indexOf("connection.dispose();");
  const retire = dispose.indexOf("retire(last)");
  assert.ok(
    flush !== -1 && retire !== -1 && flush < retire,
    "connection.dispose() runs before the provider is retired",
  );
  const retireFn = between(src, "function retire(", "function bindSession(");
  assert.match(retireFn, /generation\.scope\.stop\(\);/);
  assert.match(
    retireFn,
    /window\.setTimeout\(\(\) => \{\s*generation\.provider\.destroy\(\);\s*\}, 0\);/,
  );
});

await test("useCollabRoom flushes on pagehide and re-asserts presence on pageshow", () => {
  const session = between(source(), "function bindSession(", "bindGeneration(connection.state);");
  assert.match(session, /window\.addEventListener\("pagehide", onPageHide\);/);
  assert.match(session, /provider\.flushPendingUpdates\(\);/);
  assert.match(session, /reassertPresence\(awareness, next, lastBlockId, lastTitleEditing\);/);
  assert.match(session, /if \(!peersEqual\(peers\.value, nextPeers\)\) peers\.value = nextPeers;/);
});

// Run the actual composable and persist barrier with a controlled provider
// transport. This witnesses scope disposal BEFORE delayed provider destruction.
function roomHarness(withAuthorization = false, initialWritable: boolean | null = false) {
  const providers: Provider[] = [];
  let reclaims = 0;
  class Socket extends EventEmitter {
    status = "connected";
    webSocket: object | null = {};
    disconnects = 0;
    connects = 0;
    disconnect() {
      this.disconnects++;
    }
    connect() {
      this.connects++;
      this.webSocket = {};
      this.status = "connecting";
      for (const provider of providers) provider.emit("status");
      return Promise.resolve();
    }
    close() {
      this.webSocket = null;
      this.status = "disconnected";
      for (const provider of providers) {
        provider.isAuthenticated = false;
        provider.emit("status");
        provider.emit("disconnect");
      }
      this.emit("disconnect");
    }
  }
  const socket = new Socket();
  class Provider extends EventEmitter {
    configuration: { name: string; websocketProvider: Socket };
    synced = true;
    isAuthenticated = false;
    authorizedScope = "";
    awareness = null;
    payloads: string[] = [];
    constructor(config: Provider["configuration"]) {
      super();
      this.configuration = config;
      providers.push(this);
    }
    attach() {}
    flushPendingUpdates() {}
    sendStateless(payload: string) {
      this.payloads.push(payload);
    }
    setAwarenessField() {}
    destroy() {
      this.emit("destroy");
    }
    authenticate(scope: string) {
      socket.status = "connected";
      this.emit("status");
      this.isAuthenticated = true;
      this.authorizedScope = scope;
      this.emit("authenticated", { scope });
    }
  }
  const scope = Vue.effectScope();
  const user = Vue.shallowRef<model.CollabUser | null>(model.collabUserOf("actor-A", "A"));
  const authorization = Vue.shallowRef<{
    roomName: string;
    actorId: string;
    sessionId: string;
    writable: boolean | null;
  } | null>({
    roomName: "ws:document:A",
    actorId: "actor-A",
    sessionId: "credential-A",
    writable: initialWritable,
  });
  let script = readFileSync(roomPath, "utf8");
  const parsed = ts.createSourceFile(
    "room.ts",
    script,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  for (const statement of [...parsed.statements].reverse())
    if (ts.isImportDeclaration(statement))
      script = script.slice(0, statement.getFullStart()) + script.slice(statement.end);
  script = script.replace(/export /g, "");
  const room = scope.run(() => {
    const result: unknown = runInNewContext(
      new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${script}\nreturn useCollabRoom('ws:document:A', user, authorization);})()`,
      ),
      {
        ...Vue,
        ...model,
        ...ack,
        Y,
        user,
        authorization: withAuthorization ? authorization : undefined,
        AbortController,
        FVOCI_YDOC_FRAGMENT: "body",
        HocuspocusProvider: Provider,
        createRefusalAwareSocket: () => ({ status: "connected" }),
        RoomConnection: class {
          state = { socket, generation: 0, refusal: null };
          authenticated() {}
          reclaim() {
            reclaims++;
          }
          dispose() {}
        },
        window: {
          location: { protocol: "http:", host: "localhost" },
          setTimeout,
          addEventListener() {},
          removeEventListener() {},
        },
      },
    );
    assert.ok(typeof result === "object" && result !== null && "session" in result);
    return result as {
      doc: Y.Doc;
      session: Vue.ComputedRef<{
        provider: Provider;
        doc: Y.Doc;
        generation: number;
        readOnly: boolean;
        status: string;
        durableSaved: boolean;
        persistNow(): Promise<void>;
      } | null>;
    };
  });
  assert.ok(room);
  const provider = providers[0];
  assert.ok(provider);
  const session = room.session.value;
  assert.ok(session);
  function lateAck() {
    const payload = provider.payloads[0];
    assert.ok(payload);
    provider.emit("stateless", { payload: payload.replace("persist:", "persisted:") });
  }
  return {
    scope,
    user,
    provider,
    session,
    lateAck,
    room,
    socket,
    authorization,
    reclaims: () => reclaims,
    live: () => {
      const next = room.session.value;
      assert.ok(next);
      return next;
    },
  };
}

for (const retirement of [
  "dispose",
  "actor",
  "actor-aba",
  "signed-out",
  "readonly",
  "unauthorized",
  "disconnect",
]) {
  await test(`pending persist rejects immediately on ${retirement}, before a late ACK`, async () => {
    const h = roomHarness();
    try {
      const pending = h.session.persistNow();
      const rejected = assert.rejects(pending, /collab persist/);
      if (retirement === "dispose") h.scope.stop();
      if (retirement === "actor" || retirement === "actor-aba")
        h.user.value = model.collabUserOf("actor-B", "B");
      if (retirement === "actor-aba") h.user.value = model.collabUserOf("actor-A", "A");
      if (retirement === "signed-out") h.user.value = null;
      if (retirement === "readonly") h.provider.emit("authenticated", { scope: "readonly" });
      if (retirement === "unauthorized") h.provider.emit("authenticationFailed");
      if (retirement === "disconnect") {
        h.provider.configuration.websocketProvider.status = "disconnected";
        h.provider.emit("status", { status: "disconnected" });
      }
      h.lateAck();
      await rejected;
      assert.equal(h.provider.listenerCount("stateless"), 0, "persist listener is retired");
      await assert.rejects(h.session.persistNow(), /collab persist unavailable/);
      assert.equal(
        h.provider.payloads.length,
        1,
        "retired capability cannot issue another persist",
      );
    } finally {
      h.scope.stop();
    }
  });
}

await test("readonly admission requires a real permission edge and a new socket auth, preserving identity", async () => {
  const h = roomHarness(true);
  try {
    const doc = h.room.doc;
    const clientId = doc.clientID;
    let updates = 0;
    doc.on("update", () => updates++);
    h.provider.authenticate("readonly");
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    assert.equal(h.socket.disconnects, 1);
    assert.equal(h.socket.connects, 0, "wait for actual socket close");
    assert.equal(h.live().readOnly, true, "metadata is not a server grant");
    h.provider.authenticate("read-write");
    assert.equal(h.live().readOnly, true, "late auth on old socket is ignored");
    h.socket.close();
    assert.equal(h.socket.connects, 1);
    assert.equal(h.socket.listenerCount("disconnect"), 0);
    h.provider.authenticate("read-write");
    assert.equal(h.live().readOnly, false);
    assert.equal(h.live().provider, h.provider);
    assert.equal(h.live().doc, doc);
    assert.equal(doc.clientID, clientId);
    assert.equal(h.live().generation, 0);
    assert.equal(updates, 0);
    assert.equal(h.reclaims(), 0);
    const pending = h.live().persistNow();
    const payload = h.provider.payloads.at(-1);
    assert.ok(payload);
    h.provider.emit("stateless", { payload: payload.replace("persist:", "persisted:") });
    await pending;
    assert.equal(h.live().durableSaved, true);
  } finally {
    h.scope.stop();
  }
});

await test("grant before readonly authentication waits for genuine auth; readonly denial cannot loop", () => {
  const h = roomHarness(true);
  try {
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    assert.equal(h.socket.disconnects, 0);
    h.provider.authenticate("readonly");
    assert.equal(h.socket.disconnects, 1);
    h.socket.close();
    h.provider.authenticate("readonly");
    for (let i = 0; i < 3; i++) h.provider.authenticate("readonly");
    assert.equal(h.socket.disconnects, 1);
    assert.equal(h.socket.connects, 1);
    assert.equal(h.live().readOnly, true);
  } finally {
    h.scope.stop();
  }
});

await test("unknown metadata and initial writable metadata do not invent a permission edge", () => {
  const h = roomHarness(true);
  const initial = roomHarness(true, true);
  try {
    h.provider.authenticate("readonly");
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: null,
    };
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    assert.equal(h.socket.disconnects, 0);
    assert.equal(h.live().readOnly, true);
    initial.provider.authenticate("readonly");
    assert.equal(initial.socket.disconnects, 0);
    assert.equal(initial.live().readOnly, true);
  } finally {
    h.scope.stop();
    initial.scope.stop();
  }
});

for (const retirement of ["actor-aba", "credential-aba", "doc-aba", "dispose"] as const) {
  await test(`reauthorization callback and late grant retire on ${retirement}`, () => {
    const h = roomHarness(true);
    try {
      h.provider.authenticate("readonly");
      h.authorization.value = {
        roomName: "ws:document:A",
        actorId: "actor-A",
        sessionId: "credential-A",
        writable: true,
      };
      assert.equal(h.socket.listenerCount("disconnect"), 1);
      if (retirement === "actor-aba") {
        h.user.value = model.collabUserOf("actor-B", "B");
        h.user.value = model.collabUserOf("actor-A", "A");
      } else if (retirement === "credential-aba") {
        h.authorization.value = {
          roomName: "ws:document:A",
          actorId: "actor-A",
          sessionId: "credential-B",
          writable: true,
        };
        h.authorization.value = {
          roomName: "ws:document:A",
          actorId: "actor-A",
          sessionId: "credential-A",
          writable: true,
        };
      } else if (retirement === "doc-aba") {
        h.authorization.value = {
          roomName: "ws:document:B",
          actorId: "actor-A",
          sessionId: "credential-A",
          writable: true,
        };
        h.authorization.value = {
          roomName: "ws:document:A",
          actorId: "actor-A",
          sessionId: "credential-A",
          writable: true,
        };
      } else h.scope.stop();
      assert.equal(h.socket.listenerCount("disconnect"), 0);
      h.socket.close();
      h.provider.authenticate("read-write");
      h.provider.emit("authenticationFailed");
      assert.equal(h.socket.connects, 0);
      assert.equal(h.reclaims(), 0);
      if (retirement === "dispose") assert.equal(h.room.session.value, null);
      else assert.equal(h.live().readOnly, true);
    } finally {
      h.scope.stop();
    }
  });
}

await test("same-actor revocation during close cancels grant but restores transport once as readonly", () => {
  const h = roomHarness(true);
  try {
    h.provider.authenticate("readonly");
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: false,
    };
    h.socket.close();
    h.provider.authenticate("readonly");
    assert.equal(h.socket.connects, 1);
    assert.equal(h.socket.disconnects, 1);
    assert.equal(h.live().readOnly, true);
    assert.equal(h.reclaims(), 0);
  } finally {
    h.scope.stop();
  }
});

await test("reauthentication failure stays unauthorized without reclaim; unrelated collision still reclaims", () => {
  const h = roomHarness(true);
  const normal = roomHarness();
  try {
    h.provider.authenticate("readonly");
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    h.socket.close();
    h.provider.emit("authenticationFailed");
    h.provider.emit("authenticationFailed");
    assert.equal(h.reclaims(), 0);
    assert.equal(h.live().status, "unauthorized");
    assert.equal(h.live().readOnly, true);
    h.provider.authenticate("read-write");
    assert.equal(h.live().readOnly, true, "failure retires grant authority");
    normal.provider.emit("authenticationFailed");
    assert.equal(normal.reclaims(), 1);
  } finally {
    h.scope.stop();
    normal.scope.stop();
  }
});

await test("credential change rejects in-flight persist and late old ACK even for same actor", async () => {
  const h = roomHarness(true, true);
  try {
    h.provider.authenticate("read-write");
    const pending = h.session.persistNow();
    const rejected = assert.rejects(pending, /collab persist/);
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-B",
      writable: true,
    };
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    h.lateAck();
    await rejected;
    assert.equal(h.live().durableSaved, false);
    assert.equal(h.live().readOnly, true);
    await assert.rejects(h.session.persistNow(), /collab persist unavailable/);
    assert.equal(h.provider.payloads.length, 1);
    assert.equal(h.socket.disconnects, 0);
  } finally {
    h.scope.stop();
  }
});

await test("definitive metadata denial aborts old persist and rejects late ACK without claiming transport", async () => {
  const h = roomHarness(true, true);
  try {
    h.provider.authenticate("read-write");
    const pending = h.session.persistNow();
    const rejected = assert.rejects(pending, /collab persist/);
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: false,
    };
    h.lateAck();
    await rejected;
    assert.equal(h.live().durableSaved, false);
    await assert.rejects(h.session.persistNow(), /collab persist unavailable/);
    assert.equal(h.provider.payloads.length, 1);
    assert.equal(h.socket.disconnects, 0);
    assert.equal(h.socket.connects, 0);
    assert.equal(h.reclaims(), 0);
    h.authorization.value = {
      roomName: "ws:document:A",
      actorId: "actor-A",
      sessionId: "credential-A",
      writable: true,
    };
    const fresh = h.live().persistNow();
    const freshPayload = h.provider.payloads.at(-1);
    assert.ok(freshPayload);
    assert.notEqual(freshPayload, h.provider.payloads[0]);
    h.lateAck();
    assert.equal(h.live().durableSaved, false, "old ACK cannot satisfy the new request");
    h.provider.emit("stateless", {
      payload: freshPayload.replace("persist:", "persisted:"),
    });
    await fresh;
    assert.equal(h.live().durableSaved, true);
  } finally {
    h.scope.stop();
  }
});

await test("ordinary network disconnect does not trigger extra reauthentication or reclaim", () => {
  const h = roomHarness(true);
  try {
    h.provider.authenticate("read-write");
    h.socket.close();
    h.provider.authenticate("read-write");
    assert.equal(h.socket.disconnects, 0);
    assert.equal(h.socket.connects, 0, "ordinary SDK reconnect is not owned by permission watcher");
    assert.equal(h.reclaims(), 0);
    assert.equal(h.live().readOnly, false);
  } finally {
    h.scope.stop();
  }
});
