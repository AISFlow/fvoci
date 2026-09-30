import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

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

test("useCollabRoom binds @hocuspocus/provider itself: one provider per socket generation", () => {
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

test("useCollabRoom keeps Yjs and provider objects raw, never reactive", () => {
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

test("useCollabRoom has no hex literals", () => {
  assert.equal(/#[0-9a-fA-F]{3,8}/.test(source()), false);
});

test("useCollabRoom hands the room's auth results to the connection state machine", () => {
  const bind = between(source(), "function bindGeneration(", "function retire(");
  assert.match(bind, /const onAuthenticated = \(\) => connection\.authenticated\(\);/);
  assert.match(bind, /const onAuthenticationFailed = \(\) => connection\.reclaim\(\);/);
  assert.match(bind, /provider\.off\("authenticated", onAuthenticated\);/);
});

test("useCollabRoom re-binds only on a new socket generation, never on a refusal", () => {
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

test("useCollabRoom decides the session status with collabStatusOf only", () => {
  const session = between(source(), "function bindSession(", "bindGeneration(connection.state);");
  assert.match(
    session,
    /status: collabStatusOf\(unauthorized\.value, room\.value\.refusal, connectionStatus\.value\),/,
  );
  assert.match(session, /pending: unsent\.value && !readOnly\.value,/);
  assert.match(session, /durableSaved: isDurablySaved\(bind\.value\.ack\),/);
});

test("useCollabRoom tears down: flush and socket first, then the provider after 0 ms", () => {
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
  assert.match(retireFn, /window\.setTimeout\(\(\) => generation\.provider\.destroy\(\), 0\);/);
});

test("useCollabRoom flushes on pagehide and re-asserts presence on pageshow", () => {
  const session = between(source(), "function bindSession(", "bindGeneration(connection.state);");
  assert.match(session, /window\.addEventListener\("pagehide", onPageHide\);/);
  assert.match(session, /provider\.flushPendingUpdates\(\);/);
  assert.match(session, /reassertPresence\(awareness, next, lastBlockId, lastTitleEditing\);/);
  assert.match(session, /if \(!peersEqual\(peers\.value, nextPeers\)\) peers\.value = nextPeers;/);
});
