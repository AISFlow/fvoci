import assert from "node:assert/strict";
import { runInThisContext } from "node:vm";
import ts from "typescript";
import type { RenderFunction, SetupContext } from "vue";

export function record(value: unknown): Record<string, unknown> {
  assert.ok(typeof value === "object" && value !== null && !Array.isArray(value));
  return value as Record<string, unknown>;
}

/** Execute our compiler output, then check the module and component boundaries. */
export function evaluate(code: string, imports: Record<string, unknown>): Record<string, unknown> {
  const js = ts.transpileModule(code, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const module: { exports: unknown } = { exports: {} };
  const execute: unknown = runInThisContext(`(function(require, module, exports) {${js}\n})`);
  assert.equal(typeof execute, "function");
  Reflect.apply(execute as (...args: unknown[]) => unknown, undefined, [
    (name: string) => {
      assert.ok(name in imports, `unmapped component import: ${name}`);
      return imports[name];
    },
    module,
    module.exports,
  ]);
  return record(module.exports);
}

type Setup = (props: Record<string, unknown>, context: SetupContext) => Record<string, unknown>;
export interface CompiledComponent {
  setup: Setup;
  render?: RenderFunction;
}

function isCompiledComponent(value: unknown): value is CompiledComponent {
  if (typeof value !== "object" || value === null) return false;
  const setup: unknown = Reflect.get(value, "setup");
  return typeof setup === "function";
}

export function compiledComponent(value: unknown): CompiledComponent {
  assert.ok(isCompiledComponent(value), "compiled component has setup");
  return value;
}

export function renderFunction(value: unknown): RenderFunction {
  assert.equal(typeof value, "function", "compiled template exports render");
  return value as RenderFunction;
}

export async function callCopy(state: unknown): Promise<void> {
  const onCopy = record(state).onCopy;
  assert.equal(typeof onCopy, "function", "actual setup exposes the copy handler");
  const result: unknown = Reflect.apply(onCopy as (...args: never[]) => unknown, undefined, []);
  await result;
}
