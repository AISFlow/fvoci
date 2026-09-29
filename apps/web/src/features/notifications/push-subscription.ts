/**
 * Browser push helpers for {@link PushToggle}, kept free of React and DOM
 * globals so they run under `bun test`.
 */

export const SW_URL = "/sw.js";

/** Reasons the toggle cannot be used. Only `blocked`/`failed` come from an attempt. */
export type PushBlocker = "unsupported" | "unavailable" | "blocked" | "failed";
export type PushAttempted = "blocked" | "failed";

/** `applicationServerKey` takes raw bytes; `/instance` gives unpadded base64url. */
export function decodeKey(base64url: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64url.replace(/-/g, "+").replace(/_/g, "/"));
  return new Uint8Array([...binary].map((char) => char.charCodeAt(0)));
}

/**
 * A subscription is bound to the `applicationServerKey` it was created with.
 * After `fvoci-migrate --rotate-vapid` the old browser subscription still
 * exists but every send is rejected, so it has to be replaced.
 */
export function boundTo(key: ArrayBuffer | null | undefined, publicKey: string): boolean {
  if (!key) return false;
  const expected = decodeKey(publicKey);
  const actual = new Uint8Array(key);
  return (
    actual.length === expected.length && actual.every((byte, at) => byte === expected[at])
  );
}

export interface PushSubscriptionPayload {
  endpoint: string;
  keys: { p256dh: string; auth: string };
}

/** `toJSON()` also carries `expirationTime`; the API body is strict. */
export function subscriptionBody(json: PushSubscriptionJSON): PushSubscriptionPayload {
  const { endpoint, keys } = json;
  if (!endpoint || !keys?.p256dh || !keys.auth) {
    throw new Error("incomplete subscription");
  }
  return { endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } };
}

export class PermissionBlocked extends Error {
  constructor() {
    super("blocked");
  }
}

/** Only a refused permission is "blocked"; everything else is "failed". */
export function attempted(err: unknown): PushAttempted {
  return err instanceof PermissionBlocked ? "blocked" : "failed";
}

/**
 * Support is known at once; the public key arrives later, so "unavailable"
 * only shows after `/instance` answered without a key.
 */
export function pushBlocker(input: {
  supported: boolean;
  instanceLoaded: boolean;
  publicKey: string | null;
  attempted: PushAttempted | null;
}): PushBlocker | null {
  if (!input.supported) return "unsupported";
  if (input.instanceLoaded && input.publicKey === null) return "unavailable";
  return input.attempted;
}

/** Which signed-in user created this browser's subscription (per browser profile). */
export const PUSH_OWNER_KEY = "fvoci.push.owner";

export function readPushOwner(storage: Pick<Storage, "getItem"> | null): string | null {
  try {
    return storage?.getItem(PUSH_OWNER_KEY) ?? null;
  } catch {
    return null;
  }
}

export function writePushOwner(
  storage: Pick<Storage, "setItem" | "removeItem"> | null,
  userId: string | null,
): void {
  try {
    if (userId === null) storage?.removeItem(PUSH_OWNER_KEY);
    else storage?.setItem(PUSH_OWNER_KEY, userId);
  } catch {
    /* storage unavailable: the toggle then treats the subscription as foreign */
  }
}

export interface LogoutPushDeps<R> {
  /** This browser's push endpoint, or null (no support, no subscription, error). */
  currentEndpoint: () => Promise<string | null>;
  /** The logout request; throws on network failure like `api.POST`. */
  postLogout: (pushEndpoint: string | null) => Promise<R>;
  isOk: (result: R) => boolean;
  /** Browser-side unsubscribe; may reject. */
  unsubscribe: () => Promise<void>;
  clearOwner: () => void;
}

/**
 * Logout that disconnects this browser's push. The server removes the row in
 * the logout transaction (bound session + reported endpoint); unsubscribing
 * afterwards also stops messages the push service still holds. A failed
 * endpoint lookup or unsubscribe never blocks the logout, and a failed logout
 * keeps the subscription of the still signed-in user.
 */
export async function logoutWithPushDisconnect<R>(deps: LogoutPushDeps<R>): Promise<R> {
  const endpoint = await deps.currentEndpoint().catch(() => null);
  const result = await deps.postLogout(endpoint);
  if (deps.isOk(result)) {
    deps.clearOwner();
    await deps.unsubscribe().catch(() => undefined);
  }
  return result;
}

/** Resolves `fallback` when `promise` does not settle within `ms`. */
export function withTimeout<T>(promise: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}
