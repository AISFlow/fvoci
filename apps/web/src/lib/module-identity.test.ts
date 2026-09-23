import assert from "node:assert/strict";
import { realpathSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import * as React from "react";
import * as Y from "yjs";

const webRequire = createRequire(import.meta.url);

function resolved(req: NodeJS.Require, spec: string): string {
  return realpathSync(req.resolve(spec));
}

test("web, editor, and provider resolve one react and one yjs", async () => {
  const editorReq = createRequire(
    webRequire.resolve("@fvoci/editor/fvoci-editor"),
  );
  const collabReq = createRequire(
    webRequire.resolve("@fvoci/editor/collab-tiptap"),
  );
  const providerReq = createRequire(webRequire.resolve("@hocuspocus/provider"));
  const providerReactReq = createRequire(
    webRequire.resolve("@hocuspocus/provider-react"),
  );

  const webReact = resolved(webRequire, "react");
  const webYjs = resolved(webRequire, "yjs");

  assert.equal(resolved(editorReq, "react"), webReact);
  assert.equal(resolved(providerReactReq, "react"), webReact);
  assert.equal(resolved(collabReq, "yjs"), webYjs);
  assert.equal(resolved(providerReq, "yjs"), webYjs);

  const collab = await import("@fvoci/editor/collab-tiptap");
  const doc = collab.tiptapJsonToYDoc({ type: "doc", content: [] });
  assert.equal(doc instanceof Y.Doc, true);
  assert.equal(React.createElement("div").type, "div");
});
