import { isVueAppPath } from "@/app-boundary";

// Each app is its own chunk; a page loads exactly one of them. The glob keeps
// the app entries out of each other's type-check projects (tsconfig.app.json
// checks React, tsconfig.vue.json checks Vue) while Vite still bundles both.
//
// Known cost while the two apps coexist: index.html can no longer preload the
// React entry and its stylesheet, so a cold load of a React page fetches this
// module first and then the React app, one more round trip than before (about
// 150 ms at a 150 ms RTT, a few ms locally). Preloading React from index.html
// would load React's JavaScript on the Vue pages too, and its stylesheet would
// apply there; the cost goes away with this module once React is removed.
const apps = import.meta.glob(["./main.tsx", "./vue/main.ts"]);
const entry = isVueAppPath(window.location.pathname) ? "./vue/main.ts" : "./main.tsx";
void apps[entry]!();
