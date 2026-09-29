/**
 * Waiting for the push service worker to become active, kept free of DOM
 * globals so it runs under `bun test`.
 *
 * `register()` resolves while the new worker is still installing, and
 * `PushManager.subscribe` rejects without an active worker ("no active
 * Service Worker"). `navigator.serviceWorker.ready` is not used: it never
 * rejects, so a worker that turns redundant would leave the toggle busy.
 */

/** Bounds a stuck install/activate; not a retry. */
export const ACTIVATION_TIMEOUT_MS = 10_000;

type Worker = Pick<ServiceWorker, "state" | "addEventListener" | "removeEventListener">;

export interface ActivatingRegistration {
  active: unknown;
  installing: Worker | null;
  waiting: Worker | null;
}

/**
 * Resolves once `registration` has an active worker. Rejects when its
 * installing/waiting worker turns redundant (failed install or activation),
 * when there is no worker at all, or after `timeoutMs`.
 */
export function whenActive(
  registration: ActivatingRegistration,
  timeoutMs: number = ACTIVATION_TIMEOUT_MS,
): Promise<void> {
  if (registration.active) return Promise.resolve();
  const worker = registration.installing ?? registration.waiting;
  if (!worker) return Promise.reject(new Error("no service worker"));
  return new Promise((resolve, reject) => {
    const settle = (err: Error | null) => {
      clearTimeout(timer);
      worker.removeEventListener("statechange", onChange);
      if (err) reject(err);
      else resolve();
    };
    // The registration's active worker is set before the state becomes
    // "activating", so either later state is enough for subscribe().
    const onChange = () => {
      if (registration.active) settle(null);
      else if (worker.state === "redundant") settle(new Error("service worker redundant"));
    };
    const timer = setTimeout(() => {
      settle(new Error("service worker activation timed out"));
    }, timeoutMs);
    worker.addEventListener("statechange", onChange);
    // The state may have changed between register() and this listener.
    onChange();
  });
}

/** `pushManager.subscribe` only after the registration has an active worker. */
export async function subscribeWhenActive(
  registration: ActivatingRegistration & {
    pushManager: Pick<PushManager, "subscribe">;
  },
  options: PushSubscriptionOptionsInit,
  timeoutMs: number = ACTIVATION_TIMEOUT_MS,
): Promise<PushSubscription> {
  await whenActive(registration, timeoutMs);
  return registration.pushManager.subscribe(options);
}
