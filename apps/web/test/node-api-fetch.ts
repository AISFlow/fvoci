const NODE_API_ORIGIN = "http://fvoci.test";

/** openapi-fetch builds Request with root-relative paths; resolve them in node unit tests only. */
export function installNodeRelativeRequestShim(): () => void {
  const NativeRequest = globalThis.Request;
  globalThis.Request = new Proxy(NativeRequest, {
    construct(_target, args: [RequestInfo | URL, RequestInit?]) {
      const [input, init] = args;
      if (typeof input === "string" && input.startsWith("/")) {
        return new NativeRequest(new URL(input, NODE_API_ORIGIN), init);
      }
      return new NativeRequest(input, init);
    },
  }) as typeof Request;
  return () => {
    globalThis.Request = NativeRequest;
  };
}
