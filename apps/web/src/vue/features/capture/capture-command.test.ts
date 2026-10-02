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

test("same-user credential retirement prohibits former session settlement", () => {
  const scope = inputScope();
  scope.bind(A, "source", "first-session");
  const old = scope.capture();
  scope.bind(A, "source", "new-session");
  scope.bind(A, "source", "first-session");
  expect(scope.sameActor(old)).toBe(false);
  expect(scope.current(old)).toBe(false);
});
