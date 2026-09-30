import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "@/App";
import { startUiPreferences } from "@/lib/ui-preferences";
import "@/styles/app.css";

/** Mounts the React app (src/boot.ts calls this for the React pages). */
export function start(): void {
  startUiPreferences();
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
