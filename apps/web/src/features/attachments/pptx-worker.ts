/**
 * Dedicated worker that owns one opened deck (`pptx-client.ts` talks to it).
 * Opening and slide layout run here so a deck that is slow to parse or lay
 * out never blocks the page; the client terminates this worker on timeout,
 * cancellation and unmount.
 */
import type { PptxWorkerRequest, PptxWorkerResponse } from "./pptx-client.ts";
import { openPptx, renderSlideImage, type PptxDeck } from "./pptx-deck.ts";

type WorkerScope = {
  onmessage: ((event: MessageEvent<PptxWorkerRequest>) => void) | null;
  postMessage(message: PptxWorkerResponse): void;
};

const scope = self as unknown as WorkerScope;
let deck: PptxDeck | null = null;

scope.onmessage = ({ data }) => {
  void handle(data).then(
    (response) => {
      scope.postMessage(response);
    },
    () => {
      scope.postMessage({ type: "failed" });
    },
  );
};

async function handle(request: PptxWorkerRequest): Promise<PptxWorkerResponse> {
  if (request.type === "open") {
    // Termination, not this flag, is how the client cancels.
    const opened = await openPptx(request.bytes, () => true);
    if (opened.status !== "ok") return { type: "opened", status: opened.status };
    deck = opened.deck;
    return {
      type: "opened",
      status: "ok",
      width: deck.width,
      height: deck.height,
      slideCount: deck.slides.length,
    };
  }
  if (!deck) return { type: "failed" };
  return { type: "rendered", id: request.id, slide: renderSlideImage(deck, request.index) };
}
