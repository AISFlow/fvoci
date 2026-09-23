import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "@/App";
import "@/styles/app.css";

async function bootstrap(): Promise<void> {
  const path = window.location.pathname;
  if (path !== "/setup") {
    try {
      const response = await fetch("/api/v1/setup", { credentials: "include" });
      if (response.ok) {
        const body = (await response.json()) as { needed?: boolean };
        if (body.needed) {
          window.location.replace("/setup");
          return;
        }
      }
    } catch {
      // React app will surface API errors after mount.
    }
  }

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void bootstrap();
