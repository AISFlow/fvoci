import assert from "node:assert/strict";
import test from "node:test";
import { subscribeWhenActive, whenActive } from "./push-activation.ts";

class FakeWorker extends EventTarget {
  state: ServiceWorkerState = "installing";
  listeners = 0;
  override addEventListener(...args: Parameters<EventTarget["addEventListener"]>): void {
    this.listeners += 1;
    super.addEventListener(...args);
  }
  override removeEventListener(...args: Parameters<EventTarget["removeEventListener"]>): void {
    this.listeners -= 1;
    super.removeEventListener(...args);
  }
}

/** A registration whose worker moves through states like `register()`'s job. */
function fakeRegistration(log: string[]) {
  const worker = new FakeWorker();
  const registration = {
    active: null as FakeWorker | null,
    installing: worker as FakeWorker | null,
    waiting: null as FakeWorker | null,
    pushManager: {
      subscribe: (options?: PushSubscriptionOptionsInit) => {
        // Mirrors the browser: AbortError "no active Service Worker".
        if (!registration.active) return Promise.reject(new Error("no active Service Worker"));
        log.push(`subscribe:${String(options?.userVisibleOnly)}`);
        return Promise.resolve({
          endpoint: "https://push.example.com/x",
          expirationTime: null,
          options: { applicationServerKey: null, userVisibleOnly: true },
          getKey: () => null,
          toJSON: () => ({ endpoint: "https://push.example.com/x" }),
          unsubscribe: () => Promise.resolve(true),
        } satisfies PushSubscription);
      },
    },
  };
  const move = (state: ServiceWorkerState) => {
    worker.state = state;
    if (state === "installed") {
      registration.installing = null;
      registration.waiting = worker;
    } else if (state === "activating") {
      registration.waiting = null;
      registration.active = worker;
    } else if (state === "redundant") {
      registration.installing = null;
      registration.waiting = null;
    }
    worker.dispatchEvent(new Event("statechange"));
  };
  return { registration, worker, move };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** The toggle's order: subscribe only after activation, PUT only after subscribe. */
async function enable(
  registration: Parameters<typeof subscribeWhenActive>[0],
  log: string[],
  timeoutMs?: number,
) {
  const subscription = await subscribeWhenActive(
    registration,
    { userVisibleOnly: true },
    timeoutMs,
  );
  log.push(`put:${subscription.endpoint}`);
}

await test("delayed activation: no subscribe or PUT until the worker is active", async () => {
  const log: string[] = [];
  const { registration, worker, move } = fakeRegistration(log);
  const done = enable(registration, log, 1_000);
  await tick();
  move("installed");
  await tick();
  assert.deepEqual(log, [], "nothing before activation");
  move("activating");
  await done;
  assert.deepEqual(log, ["subscribe:true", "put:https://push.example.com/x"]);
  assert.equal(worker.listeners, 0, "statechange listener removed");
});

await test("already active registration subscribes at once", async () => {
  const log: string[] = [];
  const { registration, worker } = fakeRegistration(log);
  registration.installing = null;
  registration.active = worker;
  await enable(registration, log, 1_000);
  assert.deepEqual(log, ["subscribe:true", "put:https://push.example.com/x"]);
  assert.equal(worker.listeners, 0);
});

await test("a worker already redundant before the listener attached rejects at once", async () => {
  const log: string[] = [];
  const { registration, worker } = fakeRegistration(log);
  // Turned redundant between register() resolving and the wait starting.
  worker.state = "redundant";
  await assert.rejects(enable(registration, log, 60_000), /redundant/);
  assert.deepEqual(log, []);
  assert.equal(worker.listeners, 0);
});

await test("activation failure (redundant worker) rejects without subscribe or PUT", async () => {
  const log: string[] = [];
  const { registration, worker, move } = fakeRegistration(log);
  const done = enable(registration, log, 1_000);
  await tick();
  move("redundant");
  await assert.rejects(done, /redundant/);
  assert.deepEqual(log, []);
  assert.equal(worker.listeners, 0);
});

await test("a stuck install is bounded: rejects after the timeout, no subscribe or PUT", async () => {
  const log: string[] = [];
  const { registration, worker } = fakeRegistration(log);
  const started = Date.now();
  await assert.rejects(enable(registration, log, 20), /timed out/);
  assert.ok(Date.now() - started < 1_000);
  assert.deepEqual(log, []);
  assert.equal(worker.listeners, 0);
});

await test("no worker at all rejects immediately", async () => {
  await assert.rejects(
    whenActive({ active: null, installing: null, waiting: null }, 1_000),
    /no service worker/,
  );
});

await test("a waiting worker (installed, not yet active) is waited for", async () => {
  const log: string[] = [];
  const { registration, move } = fakeRegistration(log);
  move("installed");
  const done = whenActive(registration, 1_000);
  await tick();
  move("activating");
  await done;
});
