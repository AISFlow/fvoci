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
  assert.match(bind, /const onAuthenticationFailed = \(\) => \{\s*connection\.reclaim\(\);\s*\};/);
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
function roomHarness() {
  const providers: Provider[] = [];
  class Provider extends EventEmitter {
    configuration: { name: string; websocketProvider: { status: string } };
    synced = true;
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
  }
  const scope = Vue.effectScope();
  const user = Vue.shallowRef<model.CollabUser | null>(model.collabUserOf("actor-A", "A"));
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
        `(() => {${script}\nreturn useCollabRoom('ws:document:A', user);})()`,
      ),
      {
        ...Vue,
        ...model,
        ...ack,
        Y,
        user,
        AbortController,
        FVOCI_YDOC_FRAGMENT: "body",
        HocuspocusProvider: Provider,
        createRefusalAwareSocket: () => ({ status: "connected" }),
        RoomConnection: class {
          state = { socket: { status: "connected" }, generation: 0, refusal: null };
          authenticated() {}
          reclaim() {}
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
    return result as { session: Vue.ComputedRef<{ persistNow(): Promise<void> }> };
  });
  assert.ok(room);
  const provider = providers[0];
  assert.ok(provider);
  const session = room.session.value;
  function lateAck() {
    const payload = provider.payloads[0];
    assert.ok(payload);
    provider.emit("stateless", { payload: payload.replace("persist:", "persisted:") });
  }
  return { scope, user, provider, session, lateAck };
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
