import assert from "node:assert/strict";
import { realpathSync } from "node:fs";
import { createRequire } from "node:module";
import test from "node:test";
import * as React from "react";
import * as Vue from "vue";
import * as Y from "yjs";

const webRequire = createRequire(import.meta.url);

function resolved(req: NodeJS.Require, spec: string): string {
  return realpathSync(req.resolve(spec));
}

test("web, Vue editor, and provider share Vue, Tiptap, and Yjs identities", async () => {
  const editorReq = createRequire(
    webRequire.resolve("@fvoci/editor/vue"),
  );
  const collabReq = createRequire(
    webRequire.resolve("@fvoci/editor/collab-tiptap"),
  );
  const providerReq = createRequire(webRequire.resolve("@hocuspocus/provider"));
  const tiptapVueReq = createRequire(
    editorReq.resolve("@tiptap/vue-3"),
  );

  const webVue = resolved(webRequire, "vue");
  const webYjs = resolved(webRequire, "yjs");

  assert.equal(resolved(editorReq, "vue"), webVue);
  assert.equal(resolved(tiptapVueReq, "vue"), webVue);
  assert.equal(
    resolved(tiptapVueReq, "@tiptap/core"),
    resolved(editorReq, "@tiptap/core"),
  );
  assert.equal(resolved(collabReq, "yjs"), webYjs);
  assert.equal(resolved(providerReq, "yjs"), webYjs);

  const collab = await import("@fvoci/editor/collab-tiptap");
  const doc = collab.tiptapJsonToYDoc({ type: "doc", content: [] });
  assert.equal(doc instanceof Y.Doc, true);
  assert.equal(Vue.createVNode("div").type, "div");
});

test("the retained PDF converter and renderer resolve one React", () => {
  const pdfReq = createRequire(webRequire.resolve("@fvoci/editor/export/pdf"));
  const rendererReq = createRequire(pdfReq.resolve("@react-pdf/renderer"));
  const webReact = resolved(webRequire, "react");
  assert.equal(resolved(pdfReq, "react"), webReact);
  assert.equal(resolved(rendererReq, "react"), webReact);
  assert.equal(React.createElement("div").type, "div");
});
