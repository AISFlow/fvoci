import { HwpDocument } from "@rhwp/core";
import { createHwpSession, type HwpRequest, type HwpResponse } from "./hwp-worker-core.ts";
import { initRhwp } from "./rhwp-init.ts";

/** Module worker for one HWP/HWPX document (see `hwp-client.ts`). */
const scope = self as unknown as {
  onmessage: ((event: MessageEvent<HwpRequest>) => void) | null;
  postMessage(message: HwpResponse, transfer?: Transferable[]): void;
};

const handle = createHwpSession({ init: initRhwp, open: (bytes) => new HwpDocument(bytes) });
let queue = Promise.resolve();

// rhwp calls are synchronous; answering in arrival order keeps one in flight.
scope.onmessage = (event) => {
  const request = event.data;
  queue = queue.then(async () => {
    let response: HwpResponse;
    try {
      response = await handle(request);
    } catch {
      response = { id: request.id, ok: false, error: "failed" };
    }
    // Exported bytes are rhwp's own copy; hand the buffer over instead of cloning it.
    scope.postMessage(
      response,
      response.ok && response.op === "export" ? [response.bytes.buffer as ArrayBuffer] : [],
    );
  });
};
