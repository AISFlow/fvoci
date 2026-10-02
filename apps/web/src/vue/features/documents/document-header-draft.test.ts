import assert from "node:assert/strict";
import test from "node:test";
import { effectScope, nextTick, shallowRef } from "vue";
import { useDocumentHeaderDraft } from "./useDocumentHeaderDraft";

await test("same-document refresh owns dirty fields individually and adopts untouched server fields", async () => {
  const lifetime = effectScope();
  const source = shallowRef({ title: "server", icon: null as string | null, status: "draft" });
  const draft = lifetime.run(() => useDocumentHeaderDraft(() => source.value));
  assert.ok(draft);
  try {
    draft.title.value = "unsaved";
    source.value = { title: "remote title", icon: "🌱", status: "published" };
    await nextTick();
    assert.deepEqual(
      [draft.title.value, draft.icon.value, draft.status.value],
      ["unsaved", "🌱", "published"],
    );
    draft.icon.value = "📌";
    source.value = { title: "another remote title", icon: "🌿", status: "draft" };
    await nextTick();
    assert.deepEqual(
      [draft.title.value, draft.icon.value, draft.status.value],
      ["unsaved", "📌", "draft"],
    );
    assert.equal(
      source.value.title,
      "another remote title",
      "the draft must not mutate query data",
    );
    source.value = { title: "unsaved", icon: "📌", status: "draft" };
    await nextTick();
    source.value = { title: "confirmed later title", icon: null, status: "published" };
    await nextTick();
    assert.deepEqual(
      [draft.title.value, draft.icon.value, draft.status.value],
      ["confirmed later title", "", "published"],
    );
  } finally {
    lifetime.stop();
  }
});

await test("retirement discards every field and a new owner can start a fresh draft", async () => {
  const lifetime = effectScope();
  const source = shallowRef<{ title: string; icon: string | null; status: string }>();
  const draft = lifetime.run(() => useDocumentHeaderDraft(() => source.value));
  assert.ok(draft);
  try {
    source.value = { title: "A", icon: "A", status: "draft" };
    await nextTick();
    draft.title.value = "old owner title";
    draft.icon.value = "old owner icon";
    draft.status.value = "published";
    draft.reset();
    assert.deepEqual(
      [draft.title.value, draft.icon.value, draft.status.value],
      ["A", "A", "draft"],
    );
    source.value = undefined;
    draft.reset();
    assert.deepEqual([draft.title.value, draft.icon.value, draft.status.value], ["", "", "draft"]);
    source.value = { title: "B", icon: null, status: "published" };
    await nextTick();
    draft.title.value = "new owner title";
    source.value = { title: "B refetched", icon: "new icon", status: "draft" };
    await nextTick();
    assert.deepEqual(
      [draft.title.value, draft.icon.value, draft.status.value],
      ["new owner title", "new icon", "draft"],
    );
  } finally {
    lifetime.stop();
  }
});
