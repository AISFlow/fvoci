import { compileScript, parse } from "vue/compiler-sfc";

// bun test has no loader for single-file components; tests that import the
// Vue editor's modules (test/vue-node-views.test.ts) get each .vue compiled
// here the way @vitejs/plugin-vue compiles it for the build: <script setup>
// with the template inlined into its render function. Styles are ignored.

// The part of Bun's runtime API used here (the repo installs no @types/bun).
declare const Bun: {
  plugin(plugin: {
    name: string;
    setup(build: {
      onLoad(
        options: { filter: RegExp },
        load: (args: { path: string }) => Promise<{ contents: string; loader: "ts" }>,
      ): void;
    }): void;
  }): void;
  file(path: string): { text(): Promise<string> };
  hash(data: string): number | bigint;
};

Bun.plugin({
  name: "fvoci-vue-sfc",
  setup(build) {
    build.onLoad({ filter: /\.vue$/ }, async ({ path }) => {
      const { descriptor, errors } = parse(await Bun.file(path).text(), { filename: path });
      const error = errors[0];
      if (error !== undefined) throw typeof error === "string" ? new Error(error) : error;
      const script = compileScript(descriptor, {
        id: Bun.hash(path).toString(16),
        inlineTemplate: true,
        isProd: true,
      });
      return { contents: script.content, loader: "ts" };
    });
  },
});
