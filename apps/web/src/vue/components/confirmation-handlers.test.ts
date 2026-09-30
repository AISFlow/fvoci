import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { ref } from "vue";

// Execute each actual SFC handler. The browser lane covers mounted dialog/focus behavior;
// these checks cover synchronous exceptions and pending/rejected Promise ownership.
function handler(file: string, name: string, action: () => void | Promise<void>) {
  const sfc = readFileSync(new URL(file, import.meta.url), "utf8");
  const script = /<script setup lang="ts">([\s\S]*?)<\/script>/.exec(sfc)?.[1];
  assert.ok(script, `${file}: setup script exists`);
  const source = ts.createSourceFile(file, script, ts.ScriptTarget.Latest, true);
  const declaration = source.statements.find(
    (node): node is ts.FunctionDeclaration =>
      ts.isFunctionDeclaration(node) && node.name?.text === name,
  );
  assert.ok(declaration, `${file}: confirmation handler exists`);
  assert.ok(
    declaration.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.AsyncKeyword),
  );
  const busy = ref(false);
  const open = ref(true);
  const code = ts.transpileModule(`${declaration.getText(source)}\n${name};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const evaluated: unknown = runInNewContext(code, {
    busy,
    props: { onConfirm: action, action },
    close: () => {
      open.value = false;
    },
  });
  function isHandler(value: unknown): value is () => Promise<void> {
    return typeof value === "function";
  }
  assert.ok(isHandler(evaluated));
  return { run: evaluated, busy, open };
}

for (const [file, name] of [
  ["ConfirmAction.vue", "confirm"],
  ["ConfirmActionButton.vue", "onConfirm"],
] as const) {
  await test(`${file}: a synchronous action exception rejects and releases the dialog`, async () => {
    const failure = new Error("synchronous action failure");
    const state = handler(file, name, () => {
      throw failure;
    });
    await assert.rejects(state.run(), (error: unknown) => error === failure);
    assert.equal(state.busy.value, false);
    assert.equal(state.open.value, false);
  });

  await test(`${file}: a rejected action remains observable and releases the dialog`, async () => {
    const failure = new Error("asynchronous action failure");
    const state = handler(file, name, () => Promise.reject(failure));
    await assert.rejects(state.run(), (error: unknown) => error === failure);
    assert.equal(state.busy.value, false);
    assert.equal(state.open.value, false);
  });

  await test(`${file}: pending action keeps the dialog busy until it settles`, async () => {
    let finish: (() => void) | undefined;
    const action = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const state = handler(file, name, () => action);
    const done = state.run();
    assert.equal(state.busy.value, true);
    assert.equal(state.open.value, true);
    assert.ok(finish);
    finish();
    await done;
    assert.equal(state.busy.value, false);
    assert.equal(state.open.value, false);
  });
}
