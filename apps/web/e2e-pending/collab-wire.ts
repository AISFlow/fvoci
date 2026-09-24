/**
 * Bounded Hocuspocus 4.6.0 frame reader for product /collab Playwright
 * observation. Persist strings and auth token shape match
 * packages/editor/src/collab/constants.ts and the public provider 4.6.0
 * goldens in compat/fixtures/hocus-wire.json. This is not a second codec.
 */
export const COLLAB_PERSIST_REQUEST = "persist";
export const COLLAB_PERSIST_DONE = "persisted";
export const COLLAB_PERSIST_FAILED = "persist-failed";
export const PROVIDER_VERSION = "4.6.0";
export const SESSION_COOKIE = "fvoci_session";
export const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export const PERSIST_ID_RE = new RegExp(
  `^(${COLLAB_PERSIST_REQUEST}|${COLLAB_PERSIST_DONE}|${COLLAB_PERSIST_FAILED}):(${UUID_RE.source.slice(1, -1)})$`,
  "i",
);

const MAX_SAFE_UINT = Number.MAX_SAFE_INTEGER;
const MAX_STRING = 65_536;

export type CollabFrame =
  | { kind: "ping" }
  | { kind: "pong" }
  | {
      kind: "auth-token";
      routingKey: string;
      token: string;
      providerVersion: string | null;
    }
  | { kind: "auth-scope"; routingKey: string; scope: string }
  | { kind: "auth-denied"; routingKey: string; reason: string }
  | { kind: "stateless"; routingKey: string; payload: string }
  | { kind: "other"; routingKey: string; type: number };

class Cursor {
  pos = 0;
  input: Uint8Array;
  constructor(input: Uint8Array) {
    this.input = input;
  }

  remaining(): number {
    return this.input.length - this.pos;
  }

  readByte(): number {
    if (this.pos >= this.input.length) throw new Error("truncated");
    const byte = this.input[this.pos];
    this.pos += 1;
    return byte;
  }

  readVarUint(): number {
    let result = 0;
    let shift = 0;
    for (let i = 0; i < 10; i += 1) {
      const byte = this.readByte();
      result += (byte & 0x7f) * 2 ** shift;
      if (result > MAX_SAFE_UINT) throw new Error("varuint overflow");
      if ((byte & 0x80) === 0) return result;
      shift += 7;
    }
    throw new Error("varuint overflow");
  }

  readVarString(): string {
    const len = this.readVarUint();
    if (len > MAX_STRING || this.remaining() < len) throw new Error("truncated");
    const slice = this.input.subarray(this.pos, this.pos + len);
    this.pos += len;
    return new TextDecoder().decode(slice);
  }
}

export function frameBytes(payload: string | Uint8Array | ArrayBuffer): Uint8Array {
  if (typeof payload === "string") {
    const out = new Uint8Array(payload.length);
    for (let i = 0; i < payload.length; i += 1) {
      out[i] = payload.charCodeAt(i) & 0xff;
    }
    return out;
  }
  if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
  return payload;
}

export function decodeHocuspocusFrame(bytes: Uint8Array): CollabFrame | null {
  if (bytes.length === 0) return null;
  if (bytes.length === 1 && bytes[0] === 9) return { kind: "ping" };
  if (bytes.length === 1 && bytes[0] === 10) return { kind: "pong" };
  try {
    const cursor = new Cursor(bytes);
    const routingKey = cursor.readVarString();
    const type = cursor.readVarUint();
    if (type === 2) {
      const auth = cursor.readVarUint();
      if (auth === 0) {
        if (cursor.remaining() === 0) {
          return { kind: "other", routingKey, type };
        }
        const token = cursor.readVarString();
        const providerVersion = cursor.remaining() > 0 ? cursor.readVarString() : null;
        return { kind: "auth-token", routingKey, token, providerVersion };
      }
      if (auth === 1) {
        return { kind: "auth-denied", routingKey, reason: cursor.readVarString() };
      }
      if (auth === 2) {
        return { kind: "auth-scope", routingKey, scope: cursor.readVarString() };
      }
    }
    if (type === 5) {
      return { kind: "stateless", routingKey, payload: cursor.readVarString() };
    }
    return { kind: "other", routingKey, type };
  } catch {
    return null;
  }
}

export function persistParts(
  payload: string,
): { kind: "request" | "done" | "failed"; id: string } | null {
  const match = PERSIST_ID_RE.exec(payload);
  if (!match) return null;
  const prefix = match[1];
  const id = match[2];
  if (prefix === COLLAB_PERSIST_REQUEST) return { kind: "request", id };
  if (prefix === COLLAB_PERSIST_DONE) return { kind: "done", id };
  return { kind: "failed", id };
}
