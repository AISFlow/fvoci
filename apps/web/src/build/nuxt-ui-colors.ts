// Nuxt UI's colors plugin (@nuxt/ui dist/runtime/plugins/colors.js) injects a
// <style> at runtime that defines the --ui-color-* palette every component
// reads. The page CSP allows inline styles only by the hashes the server takes
// from index.html (src/http/security_headers.rs), so the build writes the same
// text into index.html and the injected copies match that hash. The text must
// stay byte-identical to what the plugin renders; nuxt-ui-colors.test.ts runs
// the installed plugin against this function.

import colors from "tailwindcss/colors";

/** The `ui` part of Nuxt UI's app config that the colors plugin reads. */
export interface NuxtUiColorConfig {
  readonly colors: Readonly<Record<string, string>>;
  readonly prefix?: string;
}

const SHADES = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950] as const;

function colorValue(color: string, shade: number): string {
  const palette = (colors as Record<string, unknown>)[color];
  if (typeof palette === "object" && palette !== null && shade in palette) {
    return (palette as Record<number, string>)[shade] ?? "";
  }
  return "";
}

function shadeVars(key: string, value: string, prefix: string | undefined): string {
  const prefixStr = prefix ? `${prefix}-` : "";
  const name = value === "neutral" ? "old-neutral" : value;
  return SHADES.map(
    (shade) => `--ui-color-${key}-${shade}: var(--${prefixStr}color-${name}-${shade}, ${colorValue(value, shade)});`,
  ).join("\n  ");
}

/** The style text Nuxt UI's colors plugin renders for `ui`. */
export function nuxtUiColorsCss(ui: NuxtUiColorConfig): string {
  const { neutral: _neutral, ...rest } = ui.colors;
  const accents = Object.keys(rest);
  return `@layer theme {
  :root, :host {
  ${Object.entries(ui.colors)
    .map(([key, value]) => shadeVars(key, value, ui.prefix))
    .join("\n  ")}
  }
  :root, :host, .light {
  ${accents.map((key) => `--ui-${key}: var(--ui-color-${key}-500);`).join("\n  ")}
  }
  .dark {
  ${accents.map((key) => `--ui-${key}: var(--ui-color-${key}-400);`).join("\n  ")}
  }
}`;
}

/**
 * The `ui` app config Nuxt UI's Vite plugins resolved from their options, read
 * from the plugin that serves it to the app (`#build/app.config`), so the
 * build uses exactly the colors the runtime gets.
 */
export async function nuxtUiAppConfig(
  plugins: readonly { name: string; resolveId?: unknown; load?: unknown }[],
): Promise<NuxtUiColorConfig> {
  const plugin = plugins.find((p) => p.name === "nuxt:ui:app-config");
  if (!plugin || typeof plugin.resolveId !== "function" || typeof plugin.load !== "function") {
    throw new Error("Nuxt UI app-config plugin not found");
  }
  const id: unknown = await plugin.resolveId.call({}, "#build/app.config");
  if (typeof id !== "string") throw new Error("Nuxt UI app config did not resolve");
  const code: unknown = await plugin.load.call({}, id);
  const json = typeof code === "string" ? /export default\s*(\{[\s\S]*\})\s*$/.exec(code.trim())?.[1] : undefined;
  if (!json) throw new Error("Nuxt UI app config module has an unexpected shape");
  const config = JSON.parse(json) as { ui?: NuxtUiColorConfig };
  if (!config.ui?.colors) throw new Error("Nuxt UI app config has no ui.colors");
  return config.ui;
}
