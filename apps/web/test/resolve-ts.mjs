import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const ts = require("typescript");

function isPackedFvociSource(url) {
  try {
    const path = fileURLToPath(url);
    return (
      path.includes("/node_modules/@fvoci/") &&
      (path.endsWith(".ts") || path.endsWith(".tsx"))
    );
  } catch {
    return false;
  }
}

/** WHY: editor sources keep source `.js` specifiers; Node strip-types does not remap them to `.ts`. */
export async function resolve(specifier, context, nextResolve) {
  if (specifier.startsWith("@/")) {
    const relative = specifier.slice(2);
    const base = new URL("../src/", import.meta.url);
    for (const suffix of ["", ".ts", ".tsx"]) {
      const candidate = new URL(`${relative}${suffix}`, base);
      if (existsSync(fileURLToPath(candidate))) {
        return { shortCircuit: true, url: candidate.href };
      }
    }
  }
  if (specifier.endsWith(".js") && context.parentURL) {
    try {
      const candidate = new URL(specifier, context.parentURL);
      const asTs = candidate.href.replace(/\.js$/, ".ts");
      const asTsx = candidate.href.replace(/\.js$/, ".tsx");
      if (existsSync(fileURLToPath(asTs))) {
        return { shortCircuit: true, url: asTs };
      }
      if (existsSync(fileURLToPath(asTsx))) {
        return { shortCircuit: true, url: asTsx };
      }
    } catch {
      // fall through to default resolution
    }
  }
  return nextResolve(specifier, context);
}

/**
 * Packed `file:` installs copy @fvoci/editor and @fvoci/i18n into node_modules.
 * Node refuses `--experimental-strip-types` there, so tests transpile those sources.
 */
export async function load(url, context, nextLoad) {
  if (url.endsWith(".json")) {
    return nextLoad(url, {
      ...context,
      importAttributes: { ...context.importAttributes, type: "json" },
    });
  }
  if (isPackedFvociSource(url)) {
    const path = fileURLToPath(url);
    const source = readFileSync(path, "utf8");
    const result = ts.transpileModule(source, {
      compilerOptions: {
        module: ts.ModuleKind.ESNext,
        target: ts.ScriptTarget.ES2022,
        jsx: ts.JsxEmit.ReactJSX,
      },
      fileName: path,
    });
    return { format: "module", source: result.outputText, shortCircuit: true };
  }
  return nextLoad(url, context);
}
