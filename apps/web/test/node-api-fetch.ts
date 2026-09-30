import { api } from "../src/lib/api";

const NODE_API_ORIGIN = "http://fvoci.test";
const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE"] as const;
let owners = 0;
let restore: (() => void) | undefined;

/** Resolve root-relative API paths even when openapi-fetch cached Request before this fixture. */
export function installNodeRelativeRequestShim(): () => void {
  if (owners === 0) {
    const NativeRequest = globalThis.Request;
    const RelativeRequest = new Proxy(NativeRequest, {
      construct(_target, args: [RequestInfo | URL, RequestInit?]) {
        const [input, init] = args;
        return new NativeRequest(
          typeof input === "string" && input.startsWith("/")
            ? new URL(input, NODE_API_ORIGIN)
            : input,
          init,
        );
      },
    });
    globalThis.Request = RelativeRequest;
    const originals = METHODS.map((method) => [method, api[method]] as const);
    for (const [method, original] of originals) {
      Object.defineProperty(api, method, {
        configurable: true,
        writable: true,
        value: new Proxy(original, {
          apply(target, receiver, args: unknown[]) {
            const [path, options] = args;
            if (options !== undefined && (typeof options !== "object" || options === null)) {
              throw new TypeError("API request options must be an object");
            }
            const result: unknown = Reflect.apply(target, receiver, [
              path,
              { ...options, Request: RelativeRequest },
            ]);
            return result;
          },
        }),
      });
    }
    restore = () => {
      globalThis.Request = NativeRequest;
      for (const [method, original] of originals) {
        Object.defineProperty(api, method, { configurable: true, writable: true, value: original });
      }
    };
  }
  owners += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    owners -= 1;
    if (owners === 0) {
      restore?.();
      restore = undefined;
    }
  };
}
