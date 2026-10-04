#!/usr/bin/env bun
/**
 * Minimal collab client for install-smoke: auth, sync handshake, apply pinned
 * engine fixtures, persist barrier, and return the projected HTTP body JSON.
 */
import { strict as assert } from "node:assert";
import { isDeepStrictEqual } from "node:util";
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
    if (authType === 1) {
      const reason = readVarString(buffer, offset);
      return { routingKey, type, auth: "denied", reason };
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

function routingKey(workspaceId, kind, resourceId) {
  return `${workspaceId}:${kind}:${resourceId}`;
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

// The server answers the token with Authenticated (scope) or PermissionDenied
// (reason); a denial fails at once with the server's reason.
async function authAndJoin(ws, routingKey, clientId) {
  ws.send(encodeAuthToken(routingKey, clientId));
  const frame = await waitForMessage(
    ws,
    (f) => f.type === 2 && (f.auth === "authenticated" || f.auth === "denied"),
    10_000,
  );
  if (!frame) {
    throw new Error("collab auth did not return authenticated");
  }
  if (frame.auth === "denied") {
    throw new Error(`collab auth denied: ${frame.reason}`);
  }
  console.error(`collab auth scope: ${frame.scope}`);
}

async function sendUpdates(ws, routingKey, updates, clientId) {
  await authAndJoin(ws, routingKey, clientId);
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

// A document's body route, or a task's detail (which carries contentJson).
async function fetchBody(baseUrl, origin, sessionCookie, workspaceId, kind, resourceId) {
  const path = kind === "task" ? `tasks/${resourceId}` : `documents/${resourceId}/body`;
  const response = await fetch(`${baseUrl}/api/v1/workspaces/${workspaceId}/${path}`, {
    headers: {
      cookie: `fvoci_session=${sessionCookie}`,
      origin,
    },
  });
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
  const taskId = args["task-id"];
  // Exactly one target: a document (default) or a task (--task-id).
  if (Boolean(documentId) === Boolean(taskId)) {
    throw new Error("pass exactly one of --document-id or --task-id");
  }
  const kind = taskId ? "task" : "document";
  const resourceId = taskId ?? documentId;
  // Pinned fixture set: delete_only (default; exact body) on a fresh body,
  // or pending_u1 (a self-contained paragraph from another client) as a
  // distinct later edit whose readback must contain its marker text.
  // Collab client id (uint32): two people must not claim the same id in one
  // room (the server refuses a recently used id of another user).
  const clientIdArg = args["client-id"] ?? "42";
  if (!/^\d+$/.test(clientIdArg) || Number(clientIdArg) > 4294967295) {
    throw new Error(`invalid --client-id ${clientIdArg}`);
  }
  const clientId = Number(clientIdArg);
  const fixture = args.fixture ?? "delete_only";
  if (!["delete_only", "pending_u1"].includes(fixture)) {
    throw new Error(`unknown --fixture ${fixture}`);
  }
  for (const [name, value] of Object.entries({
    baseUrl,
    origin,
    session,
    workspaceId,
  })) {
    if (!value) {
      throw new Error(`missing required argument for ${name}`);
    }
  }

  const wsUrl = `${baseUrl.replace(/^http/, "ws")}/collab`;
  const key = routingKey(workspaceId, kind, resourceId);
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

  const updates =
    fixture === "pending_u1"
      ? [fixtureBytes("pending_u1.v1")]
      : [fixtureBytes("delete_only_base.v1"), fixtureBytes("delete_only.v1")];
  await sendUpdates(ws, key, updates, clientId);
  ws.close();

  const body = await fetchBody(baseUrl, origin, session, workspaceId, kind, resourceId);
  const expectations = JSON.parse(
    readFileSync(join(ROOT, "crates/collab-engine/fixtures/expectations.json"), "utf8"),
  );
  if (fixture === "pending_u1") {
    const paragraph = expectations.pending.prosemirror_json_u1.content[0];
    assert.ok(
      (body.contentJson.content ?? []).some((node) => isDeepStrictEqual(node, paragraph)),
      "readback lacks the pending_u1 paragraph",
    );
  } else {
    assert.deepEqual(body.contentJson, expectations.delete_only.prosemirror_json_after);
  }
  process.stdout.write(JSON.stringify(body));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
