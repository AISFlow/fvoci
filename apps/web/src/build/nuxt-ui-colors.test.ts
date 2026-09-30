import assert from "node:assert/strict";
import test from "node:test";
import { mock } from "bun:test";
import ui from "@nuxt/ui/vite";
import { nuxtUiUserOptions } from "./nuxt-ui-options.ts";
import { nuxtUiAppConfig, nuxtUiColorsCss, type NuxtUiColorConfig } from "./nuxt-ui-colors.ts";

type HeadInput = { style: { innerHTML: { value: string } }[] };

let current: NuxtUiColorConfig | undefined;
let head: HeadInput | undefined;
await mock.module("#imports", () => ({
  defineNuxtPlugin: (plugin: () => void) => plugin,
  useAppConfig: () => ({ ui: current }),
  // Not hydrating: the plugin only registers the head entry.
  useNuxtApp: () => ({ isHydrating: false, payload: { serverRendered: true } }),
  injectHead: () => undefined,
  useHead: (input: HeadInput) => {
    head = input;
  },
}));

/** Runs the installed Nuxt UI colors plugin for `config` and returns the style text it renders. */
async function pluginCss(config: NuxtUiColorConfig): Promise<string> {
  current = config;
  head = undefined;
  // The installed plugin factory is callable; #imports is provided by the test above.
  const plugin: { default: () => void } = await import("@nuxt/ui/runtime/plugins/colors.js");
  plugin.default();
  // The plugin invokes useHead synchronously; assignments inside that callback
  // are invisible to TypeScript control-flow analysis after head = undefined.
  const renderedHead = head as HeadInput | undefined;
  const text = renderedHead?.style[0]?.innerHTML.value;
  if (text === undefined) throw new Error("colors plugin rendered no style");
  return text;
}

await test("index.html colors style is byte-identical to Nuxt UI's runtime style", async () => {
  const plugins = ui(nuxtUiUserOptions);
  const config = await nuxtUiAppConfig(Array.isArray(plugins) ? plugins.flat() : [plugins]);
  assert.deepEqual(Object.keys(config.colors), [
    "primary",
    "secondary",
    "success",
    "info",
    "warning",
    "error",
    "neutral",
  ]);
  assert.equal(nuxtUiColorsCss(config), await pluginCss(config));
});

await test("a prefix and an unknown color render as the plugin renders them", async () => {
  const config = {
    colors: { primary: "teal", brand: "no-such-color", neutral: "neutral" },
    prefix: "fv",
  };
  assert.equal(nuxtUiColorsCss(config), await pluginCss(config));
});
