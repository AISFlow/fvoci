#!/usr/bin/env node
/**
 * Minimal collab client for install-smoke: auth, sync handshake, apply pinned
 * engine fixtures, persist barrier, and return the projected HTTP body JSON.
 */
import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

function writeVarUint(out, value) {
  let remaining = BigInt(value);
  while (remaining >= 0x80n) {
    out.push(Number((remaining & 0x7fn) | 0x80n));
    remaining >>= 7n;
  }
  out.push(Number(remaining));
}

function writeVarString(out, value) {
  const bytes = Buffer.from(value, "utf8");
  writeVarUint(out, bytes.length);
  out.push(...bytes);
}

function writeVarBytes(out, value) {
  writeVarUint(out, value.length);
  out.push(...value);
}

function encodeSyncPayload(step, payload) {
  const out = [];
  writeVarUint(out, step);
  writeVarBytes(out, payload);
  return Uint8Array.from(out);
}

function encodeDocumentFrame(routingKey, messageType, payloadWriter) {
  const out = [];
  writeVarString(out, routingKey);
  writeVarUint(out, messageType);
  payloadWriter(out);
  return Buffer.from(out);
}

function encodeAuthToken(routingKey, clientId) {
  return encodeDocumentFrame(routingKey, 2, (out) => {
    writeVarUint(out, 0);
    writeVarString(out, String(clientId));
    writeVarString(out, "4.6.0");
  });
}

function encodeSync(routingKey, step, payload) {
  const yProtocol = encodeSyncPayload(step, payload);
  return encodeDocumentFrame(routingKey, 0, (out) => {
    out.push(...yProtocol);
  });
}

function encodeStateless(routingKey, body) {
  return encodeDocumentFrame(routingKey, 5, (out) => {
    writeVarString(out, body);
  });
}

function readVarUint(buffer, offset) {
  let result = 0n;
  let shift = 0;
  let pos = offset.value;
  for (let i = 0; i < 10; i += 1) {
    if (pos >= buffer.length) throw new Error("truncated varuint");
    const byte = buffer[pos++];
    result |= BigInt(byte & 0x7f) << BigInt(shift);
    if ((byte & 0x80) === 0) {
      offset.value = pos;
      return result;
    }
    shift += 7;
  }
  throw new Error("invalid varuint");
}

function readVarString(buffer, offset) {
  const len = Number(readVarUint(buffer, offset));
  const start = offset.value;
  const end = start + len;
  if (end > buffer.length) throw new Error("truncated string");
  offset.value = end;
  return buffer.subarray(start, end).toString("utf8");
}

function decodeDocumentFrame(buffer) {
  const offset = { value: 0 };
  const routingKey = readVarString(buffer, offset);
  const type = Number(readVarUint(buffer, offset));
  if (type === 2) {
    const authType = Number(readVarUint(buffer, offset));
    if (authType === 2) {
      const scope = readVarString(buffer, offset);
      return { routingKey, type, auth: "authenticated", scope };
    }
  }
  if (type === 0) {
    const step = Number(readVarUint(buffer, offset));
    return { routingKey, type, syncStep: step };
  }
  if (type === 5) {
    const body = readVarString(buffer, offset);
    return { routingKey, type, stateless: body };
  }
  if (type === 8) {
    const applied = Number(readVarUint(buffer, offset)) !== 0;
    return { routingKey, type, syncStatusApplied: applied };
  }
  return { routingKey, type };
}

function routingKey(workspaceId, documentId) {
  return `${workspaceId}:document:${documentId}`;
}

function fixtureBytes(name) {
  return readFileSync(join(ROOT, "crates/collab-engine/fixtures", name));
}

async function messageBytes(data) {
  if (data instanceof Blob) {
    return Buffer.from(await data.arrayBuffer());
  }
  return Buffer.from(data);
}

// One listener per socket queues every decoded frame in arrival order. A
// listener attached per wait drops frames that arrive between waits or while an
// earlier frame is still being decoded.
const inbox = new WeakMap();

function frameInbox(ws) {
  let box = inbox.get(ws);
  if (box) return box;
  box = { frames: [], waiters: [], closed: null, decoding: Promise.resolve() };
  const wake = () => {
    for (const waiter of box.waiters.splice(0)) waiter();
  };
  ws.addEventListener("message", (event) => {
    box.decoding = box.decoding.then(async () => {
      try {
        box.frames.push(decodeDocumentFrame(await messageBytes(event.data)));
      } catch (error) {
        box.closed = error;
      }
      wake();
    });
  });
  ws.addEventListener("close", () => {
    box.decoding = box.decoding.then(() => {
      box.closed ??= new Error("websocket closed while waiting for frame");
      wake();
    });
  });
  ws.addEventListener("error", (error) => {
    box.closed ??= error;
    wake();
  });
  inbox.set(ws, box);
  return box;
}

async function waitForMessage(ws, predicate, timeoutMs) {
  const box = frameInbox(ws);
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    while (box.frames.length > 0) {
      const frame = box.frames.shift();
      if (predicate(frame)) return frame;
    }
    if (box.closed) throw box.closed;
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, remaining);
      box.waiters.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
  throw new Error("timed out waiting for websocket frame");
}

async function completeSyncHandshake(ws, routingKey) {
  ws.send(encodeSync(routingKey, 0, Uint8Array.from([0, 0])));
  let sawStep2 = false;
  let sawServerStep1 = false;
  for (let i = 0; i < 16; i += 1) {
    const frame = await waitForMessage(ws, (f) => f.type === 0, 10_000);
    if (!frame) continue;
    if (frame.syncStep === 1 && !sawStep2) {
      sawStep2 = true;
      continue;
    }
    if (frame.syncStep === 0 && sawStep2) {
      sawServerStep1 = true;
      break;
    }
  }
  if (!sawStep2 || !sawServerStep1) {
    throw new Error("collab sync handshake incomplete");
  }
}

async function authAndJoin(ws, routingKey, clientId) {
  ws.send(encodeAuthToken(routingKey, clientId));
  const frame = await waitForMessage(
    ws,
    (f) => f.type === 2 && f.auth === "authenticated",
    10_000,
  );
  if (!frame) {
    throw new Error("collab auth did not return authenticated");
  }
}

async function sendUpdates(ws, routingKey, updates) {
  await authAndJoin(ws, routingKey, 42);
  await completeSyncHandshake(ws, routingKey);
  for (const update of updates) {
    ws.send(encodeSync(routingKey, 2, update));
  }
  const requestId = randomUUID();
  ws.send(encodeStateless(routingKey, `persist:${requestId}`));
  const persisted = await waitForMessage(
    ws,
    (f) => f.type === 5 && f.stateless === `persisted:${requestId}`,
    15_000,
  );
  if (!persisted) {
    throw new Error(`persist barrier missing persisted:${requestId}`);
  }
}

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key.startsWith("--")) continue;
    args[key.slice(2)] = value;
    i += 1;
  }
  return args;
}

async function fetchBody(baseUrl, origin, sessionCookie, workspaceId, documentId) {
  const response = await fetch(
    `${baseUrl}/api/v1/workspaces/${workspaceId}/documents/${documentId}/body`,
    {
      headers: {
        cookie: `fvoci_session=${sessionCookie}`,
        origin,
      },
    },
  );
  if (!response.ok) {
    throw new Error(`GET body failed: ${response.status} ${await response.text()}`);
  }
  return response.json();
}

async function main() {
  const args = parseArgs(process.argv);
  const baseUrl = args["base-url"];
  const origin = args.origin;
  const session = args.session;
  const workspaceId = args["workspace-id"];
  const documentId = args["document-id"];
  for (const [name, value] of Object.entries({
    baseUrl,
    origin,
    session,
    workspaceId,
    documentId,
  })) {
    if (!value) {
      throw new Error(`missing required argument for ${name}`);
    }
  }

  const wsUrl = `${baseUrl.replace(/^http/, "ws")}/collab`;
  const key = routingKey(workspaceId, documentId);
  const ws = new WebSocket(wsUrl, {
    headers: {
      cookie: `fvoci_session=${session}`,
      origin,
    },
  });
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  frameInbox(ws);

  const base = fixtureBytes("delete_only_base.v1");
  const deleteOnly = fixtureBytes("delete_only.v1");
  await sendUpdates(ws, key, [base, deleteOnly]);
  ws.close();

  const body = await fetchBody(baseUrl, origin, session, workspaceId, documentId);
  const expected = JSON.parse(
    readFileSync(
      join(ROOT, "crates/collab-engine/fixtures/expectations.json"),
      "utf8",
    ),
  ).delete_only.prosemirror_json_after;
  assert.deepEqual(body.contentJson, expected);
  process.stdout.write(JSON.stringify(body));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
