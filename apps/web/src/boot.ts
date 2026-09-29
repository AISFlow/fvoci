import { isVueAppPath } from "@/app-boundary";

// Each app is its own chunk; a page loads exactly one of them. The glob keeps
// the app entries out of each other's type-check projects (tsconfig.app.json
// checks React, tsconfig.vue.json checks Vue) while Vite still bundles both.
const apps = import.meta.glob(["./main.tsx", "./vue/main.ts"]);
const entry = isVueAppPath(window.location.pathname) ? "./vue/main.ts" : "./main.tsx";
void apps[entry]!();
