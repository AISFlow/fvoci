import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
import * as Y from "yjs";
import { getSchema } from "@tiptap/core";
import { EditorState } from "@tiptap/pm/state";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "../collab-tiptap";
import { createFvociExtensions } from "../tiptap-schema";
import { SourceModeSession } from "../source-mode";

const script = readFileSync(new URL("./FvociEditor.vue", import.meta.url), "utf8")
  .split('<script setup lang="ts">')[1]!
  .split("</script>")[0]!;
const parsed = ts.createSourceFile("FvociEditor.ts", script, ts.ScriptTarget.Latest, true);
const fn = parsed.statements.find(
  (statement) =>
    ts.isFunctionDeclaration(statement) && statement.name?.text === "restoreSourceBuffer",
)!;
const code = ts.transpile(
  `let restoredBuffer=false,sourceBase=null;${fn.getText(parsed)};restoreSourceBuffer`,
  { target: ts.ScriptTarget.ES2022 },
);

for (const changed of [false, true]) {
  test(`OFF Markdown restore ${changed ? "refuses a changed capture" : "captures exact native history"} and performs no write`, async () => {
    const doc = tiptapJsonToYDoc({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "kept" }, content: [{ type: "text", text: "initial" }] },
      ],
    });
    const base = Y.encodeStateAsUpdate(doc);
    if (changed) {
      const p = doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
      (p.get(0) as Y.XmlText).insert(7, " other changes");
    }
    const before = Y.encodeStateAsUpdate(doc),
      state = EditorState.create({
        schema: getSchema(createFvociExtensions()),
        doc: getSchema(createFvociExtensions()).nodeFromJSON(yDocToTiptapJson(doc)),
      });
    const source = new SourceModeSession(
      doc,
      () => "current-owner",
      () => true,
    );
    const capture = Vue.shallowRef(null),
      field = Vue.shallowRef({ value: "" }),
      stale = Vue.ref(false),
      dirty = Vue.ref(false),
      mode = Vue.ref("rich"),
      error = Vue.ref<string | null>(null);
    const restore = runInNewContext(code, {
      Y,
      props: { ydoc: doc, sourceBuffer: { text: "owned unapplied Markdown", baseV1: base } },
      capture,
      sourceSession: source,
      mode,
      draftDirty: dirty,
      sourceStale: stale,
      modeError: error,
      sourceField: field,
      nextTick: Vue.nextTick,
      t: (key: string) => key,
    });
    restore({ state, isDestroyed: false });
    await Vue.nextTick();
    expect(field.value.value).toBe("owned unapplied Markdown");
    expect(dirty.value).toBe(true);
    expect(mode.value).toBe("markdown");
    expect(stale.value).toBe(changed);
    expect(capture.value === null).toBe(changed);
    if (capture.value) expect(source.isCurrent(capture.value)).toBe(true);
    expect(error.value).toBe(changed ? "editor.mode.stale" : null);
    expect(Y.encodeStateAsUpdate(doc)).toEqual(before);
    source.destroy();
    doc.destroy();
  });
}
