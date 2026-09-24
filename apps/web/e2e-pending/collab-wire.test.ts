import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_FAILED,
  COLLAB_PERSIST_REQUEST,
  decodeHocuspocusFrame,
  persistParts,
  PROVIDER_VERSION,
} from "./collab-wire.ts";

const fixturePath = path.resolve(
  import.meta.dirname,
  "../../../compat/fixtures/hocus-wire.json",
);

function hexBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

test("decoder keeps provider 4.6 persist strings and awareness token", () => {
  const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as {
    pins: { "@hocuspocus/provider": string; yjs: string };
    clientID: number;
    cases: Array<{ id: string; hex: string }>;
  };
  assert.equal(fixture.pins["@hocuspocus/provider"], "4.6.0");
  assert.equal(fixture.pins.yjs, "13.6.32");
  const byId = new Map(fixture.cases.map((row) => [row.id, row.hex]));

  assert.deepEqual(decodeHocuspocusFrame(hexBytes(byId.get("connection_ping") ?? "")), {
    kind: "ping",
  });
  assert.deepEqual(decodeHocuspocusFrame(hexBytes(byId.get("connection_pong") ?? "")), {
    kind: "pong",
  });

  const auth = decodeHocuspocusFrame(hexBytes(byId.get("auth_token_client") ?? ""));
  assert.equal(auth?.kind, "auth-token");
  if (auth?.kind === "auth-token") {
    assert.equal(auth.token, String(fixture.clientID));
    assert.equal(auth.providerVersion, PROVIDER_VERSION);
    assert.notEqual(auth.token, "fvoci_session");
  }

  const readonly = decodeHocuspocusFrame(
    hexBytes(byId.get("auth_authenticated_readonly") ?? ""),
  );
  assert.equal(readonly?.kind, "auth-scope");
  if (readonly?.kind === "auth-scope") assert.equal(readonly.scope, "readonly");

  const persist = decodeHocuspocusFrame(hexBytes(byId.get("stateless_persist") ?? ""));
  assert.equal(persist?.kind, "stateless");
  if (persist?.kind === "stateless") {
    assert.deepEqual(persistParts(persist.payload), {
      kind: "request",
      id: "33333333-3333-4333-8333-333333333333",
    });
    assert.equal(persist.payload.startsWith(`${COLLAB_PERSIST_REQUEST}:`), true);
  }

  const done = decodeHocuspocusFrame(hexBytes(byId.get("stateless_persisted") ?? ""));
  assert.equal(done?.kind, "stateless");
  if (done?.kind === "stateless") {
    assert.deepEqual(persistParts(done.payload), {
      kind: "done",
      id: "33333333-3333-4333-8333-333333333333",
    });
    assert.equal(done.payload.startsWith(`${COLLAB_PERSIST_DONE}:`), true);
  }

  const failed = decodeHocuspocusFrame(
    hexBytes(byId.get("stateless_persist_failed") ?? ""),
  );
  assert.equal(failed?.kind, "stateless");
  if (failed?.kind === "stateless") {
    assert.deepEqual(persistParts(failed.payload), {
      kind: "failed",
      id: "33333333-3333-4333-8333-333333333333",
    });
    assert.equal(failed.payload.startsWith(`${COLLAB_PERSIST_FAILED}:`), true);
  }

  assert.equal(
    persistParts(`${COLLAB_PERSIST_DONE}:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa`)?.id,
    "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
  );
  assert.notEqual(
    persistParts(`${COLLAB_PERSIST_REQUEST}:33333333-3333-4333-8333-333333333333`),
    persistParts(`${COLLAB_PERSIST_DONE}:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa`),
  );
});
