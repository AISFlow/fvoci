import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** WHY: editor sources keep source `.js` specifiers; Node strip-types does not remap them to `.ts`. */
export async function resolve(specifier, context, nextResolve) {
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

export async function load(url, context, nextLoad) {
	if (url.endsWith(".json")) {
		return nextLoad(url, {
			...context,
			importAttributes: { ...context.importAttributes, type: "json" },
		});
	}
	return nextLoad(url, context);
}
