import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { PRESENCE_COLORS, presenceColorOf } from "../../lib/presence.ts";
import { collabStatusOf, collabUserOf } from "./collab-model.ts";

const sessionPath = path.join(import.meta.dirname, "../../vue/collab/useCollabRoom.ts");
const cssPath = path.join(import.meta.dirname, "../../styles/presence.css");

await test("Vue collab room binds the provider and preserves durable persist state", () => {
  const src = readFileSync(sessionPath, "utf8");
  assert.equal(src.includes("@hocuspocus/provider-react"), false);
  assert.equal(src.includes("new HocuspocusProvider"), true);
  assert.equal(src.includes("gc: false"), true);
  assert.equal(src.includes("durableSaved"), true);
  assert.equal(src.includes("syncPersistBind"), true);
  assert.equal(src.includes("scopedPersistObserver"), true);
});

await test("collab-session 에 hex 리터럴이 없다", () => {
  const src = readFileSync(sessionPath, "utf8").replace(/\/\*[\s\S]*?\*\/|\/\/.*/g, "");
  assert.equal(/#[0-9a-fA-F]{3,8}/.test(src), false);
});

await test("collabUserOf 색은 .afn-label-* --afn-label-ink 이다", () => {
  const user = collabUserOf("01a01f00-0000-7000-8000-000000000001", "김연구");
  assert.match(user.color, /^#[0-9a-fA-F]{6}$/);
  const css = readFileSync(cssPath, "utf8");
  assert.equal(css.includes(`--afn-label-ink: ${user.color}`), true);
});

const OLD_LABEL_KEYS = [
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "violet",
  "pink",
] as const;

function oldClientIndex(userId: string): number {
  const hex = userId.replaceAll("-", "").slice(-6);
  const parsed = Number.parseInt(hex, 16);
  return Number.isFinite(parsed) ? parsed % OLD_LABEL_KEYS.length : 0;
}

function labelInk(css: string, key: string): string | undefined {
  return new RegExp(`^\\.afn-label-${key} \\{[^}]*--afn-label-ink: (#[0-9a-fA-F]{6})`, "m").exec(
    css,
  )?.[1];
}

await test("presenceColorOf 는 옛 클라이언트 인덱스와 같은 .afn-label-* 잉크를 고른다", () => {
  const css = readFileSync(cssPath, "utf8");
  assert.equal(PRESENCE_COLORS.length, OLD_LABEL_KEYS.length);
  for (let i = 0; i < OLD_LABEL_KEYS.length; i += 1) {
    const userId = `01a01f00-0000-7000-8000-00000000000${i.toString(16)}`;
    const key = OLD_LABEL_KEYS[oldClientIndex(userId)] ?? "";
    assert.equal(oldClientIndex(userId), i);
    assert.equal(labelInk(css, key), PRESENCE_COLORS[i]);
    assert.equal(presenceColorOf(userId), labelInk(css, key));
  }
});

await test("다른 uuid 뒷자리는 다른 라벨 색을 고른다", () => {
  const a = collabUserOf("01a01f00-0000-7000-8000-000000000001", "김");
  const b = collabUserOf("01a01f00-0000-7000-8000-00000000000b", "박");
  assert.notEqual(a.color, b.color);
  assert.match(a.color, /^#[0-9a-fA-F]{6}$/);
  assert.match(b.color, /^#[0-9a-fA-F]{6}$/);
});

await test("collabStatusOf: unauthorized > 방 거절 > 연결 상태 순이다", () => {
  for (const connection of ["connecting", "connected", "disconnected"] as const) {
    assert.equal(
      collabStatusOf(false, null, connection),
      connection,
      "no refusal: the raw connection state",
    );
    assert.equal(collabStatusOf(false, "capacity", connection), "busy");
    assert.equal(collabStatusOf(false, "unavailable", connection), "unavailable");
    for (const refusal of [null, "capacity", "unavailable"] as const) {
      assert.equal(
        collabStatusOf(true, refusal, connection),
        "unauthorized",
        `a refusal must not hide the unauthorized note: ${String(refusal)}/${connection}`,
      );
    }
  }
});

/* Retained generation/auth/teardown contracts inspect the actual Vue glue. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\/|\/\/.*/g, "");
}

function between(src: string, start: string, end: string): string {
  const from = src.indexOf(start);
  assert.notEqual(from, -1, `missing ${start}`);
  const to = src.indexOf(end, from);
  assert.notEqual(to, -1, `missing ${end} after ${start}`);
  return src.slice(from, to);
}

await test("Vue collab room rebinds only socket generation, never refusal state", () => {
  const src = stripComments(readFileSync(sessionPath, "utf8"));
  const binding = between(src, "function bindGeneration(", "function retire(");
  assert.equal(binding.match(/new HocuspocusProvider\(/g)?.length, 1);
  assert.match(binding, /websocketProvider: state\.socket,/);
  const watch = between(src, "bindGeneration(connection.state);", "onScopeDispose(() => {");
  assert.match(watch, /\(\) => room\.value\.generation,/);
  assert.match(watch, /if \(!disposed\) bindGeneration\(room\.value\);/);
  assert.doesNotMatch(watch, /refusal/);
});

await test("Vue collab room sends authentication results to the state machine and exposes refusal in session status", () => {
  const src = stripComments(readFileSync(sessionPath, "utf8"));
  const binding = between(src, "function bindGeneration(", "function retire(");
  assert.match(binding, /onAuthenticated = \(\) => \{\s*connection\.authenticated\(\);\s*\};/);
  assert.match(binding, /onAuthenticationFailed = \(\) => \{\s*connection\.reclaim\(\);\s*\};/);
  assert.match(binding, /provider\.on\("authenticated", onAuthenticated\)/);
  assert.match(binding, /provider\.on\("authenticationFailed", onAuthenticationFailed\)/);
  assert.match(binding, /provider\.off\("authenticated", onAuthenticated\)/);
  assert.match(binding, /provider\.off\("authenticationFailed", onAuthenticationFailed\)/);
  assert.match(
    src,
    /status: collabStatusOf\(unauthorized\.value, room\.value\.refusal, connectionStatus\.value\),/,
  );
});

await test("Vue collab room computes status with collabStatusOf and releases its generation on teardown", () => {
  const src = stripComments(readFileSync(sessionPath, "utf8"));
  assert.match(src, /return computed<CollabRoomSession>\(\(\) => \(/);
  assert.match(
    src,
    /status: collabStatusOf\(unauthorized\.value, room\.value\.refusal, connectionStatus\.value\),/,
  );
  const dispose = src.slice(src.indexOf("bindGeneration(connection.state);"));
  assert.match(dispose, /disposed = true;/);
  assert.match(dispose, /connection\.dispose\(\);/);
  assert.match(dispose, /current\.value = null;/);
  assert.match(dispose, /if \(last\) retire\(last\);/);
});
