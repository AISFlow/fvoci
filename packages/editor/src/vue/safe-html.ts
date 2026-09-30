// Small HTML-rendering entry for non-editor pages. Importing the editor barrel
// also initializes the editor extensions and their collaboration dependencies.
export { default as SafeHtml } from "./SafeHtml.vue";
export { asSafeHtml, type SafeHtml as SafeHtmlString } from "../safe-html.js";
