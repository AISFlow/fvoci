import assert from "node:assert/strict";
import test from "node:test";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_FAILED,
  COLLAB_PERSIST_REQUEST,
} from "@fvoci/editor/collab";
import type { HocuspocusProvider, onStatelessParameters } from "@hocuspocus/provider";
import { persistNow } from "./collab-model.ts";

function fakeProvider(): {
  provider: HocuspocusProvider;
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
  } as unknown as HocuspocusProvider;
  return {
    provider,
    calls,
    emit(payload: string) {
      for (const listener of listeners) listener({ payload });
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
