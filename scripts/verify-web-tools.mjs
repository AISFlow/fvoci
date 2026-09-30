import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const expected = {
  eslint: "10.11.0",
  "@eslint/js": "10.0.1",
  "typescript-eslint": "8.71.0",
  "@typescript-eslint/parser": "8.71.0",
  "eslint-plugin-vue": "10.11.1",
  "vue-eslint-parser": "10.4.1",
  prettier: "3.9.9",
  "eslint-config-prettier": "10.1.8",
  globals: "17.12.0",
  typescript: "5.9.3",
  "@types/bun": "1.4.2",
};
const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
if (process.versions.bun !== "1.4.2") throw new Error("Web checks require pinned Bun 1.4.2");
for (const [name, version] of Object.entries(expected)) {
  const installed = require(`${name}/package.json`).version;
  if (manifest.devDependencies[name] !== version || installed !== version) {
    throw new Error(
      `Expected ${name}@${version}; manifest=${manifest.devDependencies[name]}, installed=${installed}`,
    );
  }
}
