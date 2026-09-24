import assert from "node:assert/strict";
import test from "node:test";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_FAILED,
  COLLAB_PERSIST_REQUEST,
} from "@fvoci/editor/collab";
import {
  HocuspocusProvider,
  HocuspocusProviderWebsocket,
  type HocuspocusProvider as Provider,
  type onDisconnectParameters,
  type onStatelessParameters,
} from "@hocuspocus/provider";
import * as Y from "yjs";
import {
  PERSIST_DISCONNECTED_MESSAGE,
  persistNow,
} from "./collab-model.ts";

function fakeProvider(): {
  provider: Provider;
  calls: string[];
  emit(payload: string): void;
} {
  const listeners = new Set<(params: onStatelessParameters) => void>();
  const calls: string[] = [];
  const provider = {
    flushPendingUpdates() {
      calls.push("flush");
    },
    sendStateless(payload: string) {
      calls.push(`stateless:${payload}`);
    },
    on(event: string, fn: (params: onStatelessParameters) => void) {
      if (event === "stateless") listeners.add(fn);
      return provider;
    },
    off(event: string, fn: (params: onStatelessParameters) => void) {
      if (event === "stateless") listeners.delete(fn);
      return provider;
    },
  } as unknown as Provider;
  return {
    provider,
    calls,
    emit(payload: string) {
      for (const listener of listeners) listener({ payload });
    },
  };
}

function requestIdOf(payloads: string[]): string {
  const line = payloads.find((payload) =>
    payload.startsWith(`${COLLAB_PERSIST_REQUEST}:`),
  );
  const id = line?.slice(`${COLLAB_PERSIST_REQUEST}:`.length);
  assert.ok(id);
  return id;
}

function liveProvider(name = "workspace:document:doc"): {
  doc: Y.Doc;
  provider: HocuspocusProvider;
  websocket: HocuspocusProviderWebsocket;
  payloads: string[];
  close(): void;
  destroy(): void;
} {
  const doc = new Y.Doc({ gc: false });
  const websocket = new HocuspocusProviderWebsocket({
    url: "ws://127.0.0.1:9",
    autoConnect: false,
  });
  const provider = new HocuspocusProvider({
    name,
    document: doc,
    websocketProvider: websocket,
  });
  provider.attach();
  const payloads: string[] = [];
  const send = provider.sendStateless.bind(provider);
  provider.sendStateless = (payload: string) => {
    payloads.push(payload);
    send(payload);
  };
  return {
    doc,
    provider,
    websocket,
    payloads,
    close() {
      websocket.onClose({
        event: {
          code: 1006,
          reason: "test-disconnect",
          wasClean: false,
        } as onDisconnectParameters["event"],
      });
    },
    destroy() {
      provider.destroy();
      websocket.destroy();
      doc.destroy();
    },
  };
}

test("persistNow 는 배칭된 편집을 먼저 내보내고 요청별 응답을 기다린다", async () => {
  const fake = fakeProvider();
  const persisted = persistNow(fake.provider);
  assert.equal(fake.calls[0], "flush");
  assert.match(fake.calls[1] ?? "", new RegExp(`^stateless:${COLLAB_PERSIST_REQUEST}:[0-9a-f-]{36}$`));
  const requestId = fake.calls[1]?.slice(`stateless:${COLLAB_PERSIST_REQUEST}:`.length);
  fake.emit(`${COLLAB_PERSIST_DONE}:${requestId}`);
  await persisted;
});

test("persist-failed 는 성공으로 접히지 않는다", async () => {
  const fake = fakeProvider();
  const persisted = persistNow(fake.provider);
  const requestId = fake.calls[1]?.slice(`stateless:${COLLAB_PERSIST_REQUEST}:`.length);
  fake.emit(`${COLLAB_PERSIST_FAILED}:${requestId}`);
  await assert.rejects(persisted, /collab persist failed/);
});

test("다른 요청 id 의 persisted 응답은 이 저장을 끝내지 않는다", async () => {
  const fake = fakeProvider();
  const persisted = persistNow(fake.provider);
  fake.emit(`${COLLAB_PERSIST_DONE}:${crypto.randomUUID()}`);
  const requestId = fake.calls[1]?.slice(`stateless:${COLLAB_PERSIST_REQUEST}:`.length);
  fake.emit(`${COLLAB_PERSIST_DONE}:${requestId}`);
  await persisted;
});

test("timeout 은 저장 성공이 아니다", async () => {
  const fake = fakeProvider();
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  globalThis.setTimeout = ((fn: () => void) => {
    queueMicrotask(fn);
    return 0;
  }) as typeof setTimeout;
  globalThis.clearTimeout = (() => undefined) as typeof clearTimeout;
  try {
    await assert.rejects(persistNow(fake.provider), /collab persist timed out/);
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
  }
});

test("persistNow observer 는 요청 id 만 알리고 외국 ack 는 성공으로 부르지 않는다", async () => {
  const fake = fakeProvider();
  const seen: string[] = [];
  const persisted = persistNow(fake.provider, {
    onRequest: (id) => seen.push(`request:${id}`),
    onAck: (id) => seen.push(`ack:${id}`),
    onFail: (id) => seen.push(`fail:${id}`),
    onTimeout: (id) => seen.push(`timeout:${id}`),
  });
  const requestId = fake.calls[1]?.slice(`stateless:${COLLAB_PERSIST_REQUEST}:`.length);
  assert.deepEqual(seen, [`request:${requestId}`]);
  fake.emit(`${COLLAB_PERSIST_DONE}:${crypto.randomUUID()}`);
  assert.deepEqual(seen, [`request:${requestId}`]);
  fake.emit(`${COLLAB_PERSIST_DONE}:${requestId}`);
  await persisted;
  assert.deepEqual(seen, [`request:${requestId}`, `ack:${requestId}`]);
});

test("real provider persistNow succeeds only on matching persisted:<id>", async () => {
  const live = liveProvider();
  try {
    live.doc.getText("t").insert(0, "본문");
    const persisted = persistNow(live.provider);
    const requestId = requestIdOf(live.payloads);
    assert.equal(live.payloads[0], `${COLLAB_PERSIST_REQUEST}:${requestId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${crypto.randomUUID()}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${requestId}`);
    await persisted;
    assert.equal(live.doc.getText("t").toString(), "본문");
  } finally {
    live.destroy();
  }
});

test("real provider disconnect before ack rejects and delayed persisted cannot succeed", async () => {
  const live = liveProvider();
  try {
    const clientId = live.doc.clientID;
    live.doc.getText("t").insert(0, "미전송 한글");
    const pending = Y.encodeStateAsUpdate(live.doc);
    const seen: string[] = [];
    const persisted = persistNow(live.provider, {
      onAck: (id) => seen.push(`ack:${id}`),
      onAbort: (id) => seen.push(`abort:${id}`),
    });
    const requestId = requestIdOf(live.payloads);
    live.close();
    await assert.rejects(persisted, { message: PERSIST_DISCONNECTED_MESSAGE });
    assert.deepEqual(seen, [`abort:${requestId}`]);

    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${requestId}`);
    assert.deepEqual(seen, [`abort:${requestId}`]);
    assert.equal(live.doc.clientID, clientId);
    assert.equal(live.doc.getText("t").toString(), "미전송 한글");
    assert.deepEqual(Y.encodeStateAsUpdate(live.doc), pending);
  } finally {
    live.destroy();
  }
});

test("real provider reconnect old persisted ack cannot complete the aborted save", async () => {
  const live = liveProvider();
  try {
    live.doc.getText("t").insert(0, "offline");
    const first = persistNow(live.provider);
    const oldId = requestIdOf(live.payloads);
    live.close();
    await assert.rejects(first, { message: PERSIST_DISCONNECTED_MESSAGE });

    live.payloads.length = 0;
    const second = persistNow(live.provider);
    const newId = requestIdOf(live.payloads);
    assert.notEqual(newId, oldId);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${oldId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_FAILED}:${oldId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${newId}`);
    await second;
    assert.equal(live.doc.getText("t").toString(), "offline");
  } finally {
    live.destroy();
  }
});

test("real provider persist-failed and timeout still reject", async () => {
  const live = liveProvider();
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  try {
    const failed = persistNow(live.provider);
    live.provider.receiveStateless(
      `${COLLAB_PERSIST_FAILED}:${requestIdOf(live.payloads)}`,
    );
    await assert.rejects(failed, /collab persist failed/);

    live.payloads.length = 0;
    globalThis.setTimeout = ((fn: () => void) => {
      queueMicrotask(fn);
      return 0;
    }) as typeof setTimeout;
    globalThis.clearTimeout = (() => undefined) as typeof clearTimeout;
    await assert.rejects(persistNow(live.provider), /collab persist timed out/);
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
    live.destroy();
  }
});

test("independent repeated persistNow requests clean up without crossing acks", async () => {
  const live = liveProvider();
  try {
    const first = persistNow(live.provider);
    const firstId = requestIdOf(live.payloads);
    live.payloads.length = 0;
    const second = persistNow(live.provider);
    const secondId = requestIdOf(live.payloads);
    assert.notEqual(firstId, secondId);

    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${firstId}`);
    await first;
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${firstId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${secondId}`);
    await second;

    live.payloads.length = 0;
    const third = persistNow(live.provider);
    const thirdId = requestIdOf(live.payloads);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${firstId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${secondId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${thirdId}`);
    await third;
  } finally {
    live.destroy();
  }
});

test("aborting one persistNow leaves the other request and Y.Doc intact", async () => {
  const live = liveProvider();
  try {
    live.doc.getText("t").insert(0, "남겨둘 편집");
    const abort = new AbortController();
    const first = persistNow(live.provider, undefined, { signal: abort.signal });
    const firstId = requestIdOf(live.payloads);
    live.payloads.length = 0;
    const second = persistNow(live.provider);
    const secondId = requestIdOf(live.payloads);
    abort.abort();
    await assert.rejects(first, { message: PERSIST_DISCONNECTED_MESSAGE });
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${firstId}`);
    live.provider.receiveStateless(`${COLLAB_PERSIST_DONE}:${secondId}`);
    await second;
    assert.equal(live.doc.getText("t").toString(), "남겨둘 편집");
  } finally {
    live.destroy();
  }
});
