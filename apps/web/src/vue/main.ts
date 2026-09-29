import ui from "@nuxt/ui/vue-plugin";
import { QueryClient, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp } from "vue";
import App from "./App.vue";
import { createAppRouter } from "./router";
import "./styles.css";

// The React app's query defaults (src/App.tsx).
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
    },
  },
});

const root = document.getElementById("root")!;
root.classList.add("isolate");

createApp(App).use(createAppRouter()).use(ui).use(VueQueryPlugin, { queryClient }).mount(root);
