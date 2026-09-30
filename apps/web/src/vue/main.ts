import ui from "@nuxt/ui/vue-plugin";
import { QueryClient, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp } from "vue";
import { startUiPreferences } from "@/lib/ui-preferences";
import App from "./App.vue";
import { createAppRouter } from "./router";
import "./styles.css";

/** Mounts the app from the single Vue entry. */
export function start(): void {
  startUiPreferences();
  // Shared query defaults for all app routes.
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        refetchOnWindowFocus: false,
      },
    },
  });

  const root = document.getElementById("root");
  if (!root) throw new Error("FVOCI app root element is missing");
  root.classList.add("isolate");

  createApp(App).use(createAppRouter()).use(ui).use(VueQueryPlugin, { queryClient }).mount(root);
}
