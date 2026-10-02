import { expect, test } from "bun:test";
import { effectScope, ref, watch } from "vue";
import {
  createSourceDraftRetirement,
  useSourceDraftGuard,
  type SourceDraftState,
  type SourceDraftAuthScope,
} from "./useSourceDraftGuard";

function fixture(hostWatchesFirst = false) {
  const scope = effectScope();
  const lifetime = ref(1);
  const identity = ref("actor:session:source:provider1");
  const authorized = ref(true);
  let discards = 0;
  let state: SourceDraftState = {
    owner: {},
    phase: "activate",
    scope: 1,
    dirty: true,
    stale: false,
    composing: false,
  };
  const editor = {
    get sourceDraftState() {
      return state;
    },
    discardSourceDraft() {
      discards++;
      state = { ...state, dirty: false, stale: false };
    },
  };
  if (hostWatchesFirst)
    scope.run(() =>
      watch(
        [identity, authorized],
        () => {
          lifetime.value++;
        },
        { flush: "sync" },
      ),
    );
  const guard = scope.run(() =>
    useSourceDraftGuard({
      scope: () => lifetime.value,
      identity: () => identity.value,
      authorized: () => authorized.value,
      editor: () => editor,
    }),
  );
  if (!guard) throw new Error("missing scope");
  guard.receive(state);
  return {
    scope,
    guard,
    lifetime,
    identity,
    authorized,
    editor,
    get discards() {
      return discards;
    },
    get state() {
      return state;
    },
    update(next: Partial<SourceDraftState>) {
      state = { ...state, ...next };
      guard.receive(state);
    },
  };
}
test("Cancel retains the actual source draft and never invokes discard", async () => {
  const f = fixture();
  try {
    const leave = f.guard.requestLeave();
    expect(f.guard.open.value).toBe(true);
    f.guard.keepEditing();
    expect(await leave).toBe(false);
    expect(f.state.dirty).toBe(true);
    expect(f.discards).toBe(0);
  } finally {
    f.scope.stop();
  }
});
test("explicit Discard acts only on the captured live owner and resolves navigation", async () => {
  const f = fixture();
  try {
    const leave = f.guard.requestLeave();
    f.guard.discardAndLeave();
    expect(await leave).toBe(true);
    expect(f.discards).toBe(1);
    expect(f.guard.open.value).toBe(false);
  } finally {
    f.scope.stop();
  }
});
test("newly activated editor ignores stale owner change/retire/activate", async () => {
  const f = fixture();
  try {
    const old = f.state;
    const pending = f.guard.requestLeave();
    f.update({ owner: {}, phase: "activate" });
    expect(await pending).toBe(false);
    for (const phase of ["change", "retire", "activate"] as const)
      f.guard.receive({ ...old, phase, dirty: false });
    expect(f.guard.draft.value?.owner).toBe(f.state.owner);
    const next = f.guard.requestLeave();
    expect(f.guard.open.value).toBe(true);
    f.guard.keepEditing();
    expect(await next).toBe(false);
    expect(f.discards).toBe(0);
  } finally {
    f.scope.stop();
  }
});
for (const retirement of ["actorABA", "sourceABA", "denial"] as const) {
  test(`${retirement} retires the pending guard without blocking auth cleanup or discarding`, async () => {
    const f = fixture();
    try {
      const leave = f.guard.requestLeave();
      if (retirement === "denial") f.authorized.value = false;
      else {
        const original = f.identity.value;
        f.identity.value = "retired";
        f.identity.value = original;
        f.lifetime.value += 2;
      }
      expect(await leave).toBe(true);
      f.guard.discardAndLeave();
      expect(f.discards).toBe(0);
      expect(await f.guard.requestLeave()).toBe(true);
    } finally {
      f.scope.stop();
    }
  });
}
test("IME composition cannot be discarded by a navigation confirmation", async () => {
  const f = fixture();
  try {
    f.update({ phase: "change", composing: true });
    const leave = f.guard.requestLeave();
    f.guard.discardAndLeave();
    expect(f.discards).toBe(0);
    expect(f.guard.open.value).toBe(true);
    f.guard.keepEditing();
    expect(await leave).toBe(false);
    expect(f.state.composing).toBe(true);
  } finally {
    f.scope.stop();
  }
});
test("same-owner new scope requires a fresh matching change and never accepts the old response", async () => {
  const f = fixture();
  try {
    const old = f.state;
    const leave = f.guard.requestLeave();
    f.lifetime.value = 2;
    expect(await leave).toBe(false);
    f.guard.discardAndLeave();
    expect(f.discards).toBe(0);
    f.guard.receive(old);
    const retained = f.guard.requestLeave();
    expect(f.guard.open.value).toBe(true);
    f.guard.keepEditing();
    expect(await retained).toBe(false);
    expect(f.state.dirty).toBe(true);
    f.update({ phase: "change", scope: 2 });
    const next = f.guard.requestLeave();
    expect(f.guard.open.value).toBe(true);
    f.guard.keepEditing();
    expect(await next).toBe(false);
  } finally {
    f.scope.stop();
  }
});
test("clean source permits navigation and pending guard cleanup resolves once", async () => {
  const f = fixture();
  try {
    f.update({ phase: "change", dirty: false });
    expect(await f.guard.requestLeave()).toBe(true);
    f.update({ dirty: true });
    const leave = f.guard.requestLeave();
    expect(f.guard.requestLeave()).toBe(leave);
    f.scope.stop();
    expect(await leave).toBe(true);
    f.guard.discardAndLeave();
    expect(f.discards).toBe(0);
  } finally {
    f.scope.stop();
  }
});

test("native beforeunload protects dirty input, then auth denial and disposal take priority", () => {
  const f = fixture();
  try {
    const dirty = new Event("beforeunload", { cancelable: true });
    f.guard.beforeUnload(dirty);
    expect(dirty.defaultPrevented).toBe(true);
    f.authorized.value = false;
    const denied = new Event("beforeunload", { cancelable: true });
    f.guard.beforeUnload(denied);
    expect(denied.defaultPrevented).toBe(false);
    f.scope.stop();
    const retired = new Event("beforeunload", { cancelable: true });
    f.guard.beforeUnload(retired);
    expect(retired.defaultPrevented).toBe(false);
  } finally {
    f.scope.stop();
  }
});
test("logout retirement denies only its captured actual actor/credential/lifetime", () => {
  const current = ref<SourceDraftAuthScope>({
    actorId: "actor-a",
    credentialId: "session-a",
    workspaceId: "source",
    lifetime: 1,
  });
  const retirement = createSourceDraftRetirement(() => current.value);
  const started = retirement.capture();
  expect(retirement.denied.value).toBe(false);
  expect(retirement.retire(started)).toBe(true);
  expect(retirement.denied.value).toBe(true);
  current.value = { ...current.value, actorId: "actor-b", credentialId: "session-b", lifetime: 2 };
  expect(retirement.denied.value).toBe(false);
  expect(retirement.retire(started)).toBe(false);
  current.value = { ...started, lifetime: 3 };
  expect(retirement.denied.value).toBe(false);
  expect(retirement.retire(started)).toBe(false);
});
test("captured logout retirement cannot navigate after source switch or same-actor credential ABA", () => {
  const current = ref<SourceDraftAuthScope>({
    actorId: "actor",
    credentialId: "session",
    workspaceId: "source-a",
    lifetime: 1,
  });
  const retirement = createSourceDraftRetirement(() => current.value);
  const started = retirement.capture();
  current.value = { ...current.value, workspaceId: "source-b", lifetime: 2 };
  expect(retirement.retire(started)).toBe(false);
  current.value = { ...started, credentialId: "other", lifetime: 3 };
  current.value = { ...started, lifetime: 4 };
  expect(retirement.retire(started)).toBe(false);
  expect(retirement.denied.value).toBe(false);
});

for (const retirement of ["actor", "credential", "denial"] as const) {
  test(`forced ${retirement} wins when the actual host advances scope before the guard watcher`, async () => {
    const f = fixture(true);
    try {
      const leave = f.guard.requestLeave();
      if (retirement === "denial") f.authorized.value = false;
      else
        f.identity.value =
          retirement === "actor" ? "new-actor:session:source" : "actor:new-session:source";
      expect(f.lifetime.value).toBe(2);
      expect(await leave).toBe(true);
      expect(f.guard.open.value).toBe(false);
      expect(f.guard.draft.value).toBeNull();
      expect(f.discards).toBe(0);
      expect(await f.guard.requestLeave()).toBe(true);
    } finally {
      f.scope.stop();
    }
  });
}
