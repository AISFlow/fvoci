import assert from "node:assert/strict";
import { test } from "node:test";
import { HocuspocusProvider, HocuspocusProviderWebsocket } from "@hocuspocus/provider";
import { Doc } from "yjs";
import { attachmentNodesFromDocument } from "./collab-attachment-oracle.ts";
import { expectAwarenessTokenNotSession } from "./collab-helpers.ts";
import { decodeHocuspocusFrame, FIXTURE_PROVIDER_VERSION } from "./collab-wire.ts";

test("live auth assertion accepts the installed provider's generated frame without a network", async () => {
  class NoNetworkWebSocket {
    constructor() {
      throw new Error("auth regression must not open a WebSocket");
    }
  }
  const socket = new HocuspocusProviderWebsocket({
    url: "ws://127.0.0.1:9/collab",
    autoConnect: false,
    WebSocketPolyfill: NoNetworkWebSocket,
  });
  const doc = new Doc();
  const provider = new HocuspocusProvider({
    name: "11111111-1111-4111-8111-111111111111:document:22222222-2222-4222-8222-222222222222",
    document: doc,
    websocketProvider: socket,
    token: String(doc.clientID),
  });
  const frames: Uint8Array[] = [];
  socket.send = (frame) => { frames.push(new Uint8Array(frame)); };
  try {
    provider.attach();
    await provider.sendToken();
    assert.equal(frames.length, 1, "capture the real AuthenticationMessage only");
    const auth = decodeHocuspocusFrame(frames[0]);
    assert.equal(auth?.kind, "auth-token");
    assert.ok(auth && auth.kind === "auth-token");
    assert.equal(auth.token, String(doc.clientID));
    assert.equal(auth.routingKey, provider.configuration.name);
    await expectAwarenessTokenNotSession({ sent: [auth], received: [] }, "test-session-cookie");
    assert.notEqual(auth.providerVersion, FIXTURE_PROVIDER_VERSION);
    for (const providerVersion of [FIXTURE_PROVIDER_VERSION, null, "wrong-version"]) {
      await assert.rejects(
        expectAwarenessTokenNotSession(
          { sent: [{ ...auth, providerVersion }], received: [] },
          "test-session-cookie",
        ),
        /toBe\(expected\)/,
      );
    }
  } finally {
    provider.destroy();
    socket.destroy();
    doc.destroy();
  }
});

test("attachmentNodesFromDocument extracts stored attachment id, name, and image flag", () => {
  assert.deepEqual(
    attachmentNodesFromDocument({
      type: "doc",
      content: [
        {
          type: "attachment",
          attrs: {
            id: "33333333-3333-7333-8333-333333333333",
            name: "collab-fixture.bin",
            image: false,
          },
        },
      ],
    }),
    [
      {
        attachmentId: "33333333-3333-7333-8333-333333333333",
        name: "collab-fixture.bin",
        image: false,
      },
    ],
  );
});
