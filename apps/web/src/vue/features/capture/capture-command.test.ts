import { expect, test } from "bun:test";
import {
  forgetCommand,
  inputScope,
  recoverCommand,
  rememberCommand,
  type CommandStorage,
  type PendingInputCommand,
} from "./capture-command";
const A = "00000000-0000-4000-8000-000000000001",
  B = "00000000-0000-4000-8000-000000000002";
function storage(): CommandStorage {
  const data = new Map<string, string>();
  return {
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => {
      data.set(k, v);
    },
    removeItem: (k) => {
      data.delete(k);
    },
  };
}
const command = (): PendingInputCommand => ({
  actorId: A,
  workspaceId: B,
  body: { requestId: crypto.randomUUID(), intent: "task", title: "한글 research 🙂" },
});
test("lost success survives a new consumer with original payload and command UUID", () => {
  const s = storage(),
    first = command();
  rememberCommand(s, first);
  expect(recoverCommand(s, A)).toEqual(first);
  expect(recoverCommand(s, B)).toBeNull();
  const retry = recoverCommand(s, A);
  if (!retry) throw new Error("missing persisted retry command");
  expect(retry.body.requestId).toBe(first.body.requestId);
  forgetCommand(s, retry);
  expect(recoverCommand(s, A)).toBeNull();
});
test("a stored command with any 8-4-4-4-12 hex ids stays recoverable unchanged", () => {
  // Version and variant nibbles outside RFC 9562, which the stored ids were never checked for.
  const s = storage(),
    loose: PendingInputCommand = {
      actorId: "00000000-0000-0000-0000-000000000001",
      workspaceId: "11111111-1111-1111-1111-111111111111",
      body: {
        requestId: "0190a0c6-9a1e-0cc1-ca3b-0123456789AB",
        intent: "note",
        title: "보관된 명령",
        projectId: "22222222-2222-2222-2222-222222222222",
        source: { documentId: "33333333-3333-3333-3333-333333333333", anchor: null },
      },
    };
  rememberCommand(s, loose);
  expect(recoverCommand(s, loose.actorId)).toEqual(loose);
});
test("old success cannot erase a separately abandoned/new pending command", () => {
  const s = storage(),
    old = command();
  rememberCommand(s, old);
  forgetCommand(s, old);
  const next = command();
  rememberCommand(s, next);
  forgetCommand(s, old);
  expect(recoverCommand(s, A)).toEqual(next);
});
test("target A-B-A may settle same actor but cannot clear or navigate a new draft", () => {
  const scope = inputScope();
  scope.bind(A, "doc-A");
  const old = scope.capture();
  scope.bind(A, "doc-B");
  scope.bind(A, "doc-A");
  expect(scope.sameActor(old)).toBe(true);
  expect(scope.current(old)).toBe(false);
});
test("actor ABA and unmount prohibit old-actor cache settlement and publication", () => {
  const scope = inputScope();
  scope.bind(A, "doc");
  const old = scope.capture();
  scope.bind(B, "doc");
  scope.bind(A, "doc");
  expect(scope.sameActor(old)).toBe(false);
  expect(scope.current(old)).toBe(false);
  const current = scope.capture();
  scope.retire();
  expect(scope.sameActor(current)).toBe(false);
});

test("host disposal ends the UI lifetime but keeps the identity; retire and identity changes do not", () => {
  const scope = inputScope();
  scope.bind(A, "doc", "session");
  const captured = scope.capture();
  scope.dispose();
  expect(scope.sameActor(captured)).toBe(false);
  expect(scope.current(captured)).toBe(false);
  expect(scope.sameIdentity(captured)).toBe(true);
  const retired = inputScope();
  retired.bind(A, "doc", "session");
  const before = retired.capture();
  retired.retire();
  expect(retired.sameIdentity(before)).toBe(false);
  for (const next of [
    [B, "session"],
    [A, "other-session"],
  ] as const) {
    const changed = inputScope();
    changed.bind(A, "doc", "session");
    const old = changed.capture();
    changed.bind(next[0], "doc", next[1]);
    changed.bind(A, "doc", "session");
    expect(changed.sameIdentity(old)).toBe(false);
  }
});

test("same-user credential retirement prohibits former session settlement", () => {
  const scope = inputScope();
  scope.bind(A, "source", "first-session");
  const old = scope.capture();
  scope.bind(A, "source", "new-session");
  scope.bind(A, "source", "first-session");
  expect(scope.sameActor(old)).toBe(false);
  expect(scope.current(old)).toBe(false);
});
