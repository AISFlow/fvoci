// Adapted from fvoci/FVOCI apps/web/src/features/auth/auth-layout.tsx
import type { ReactNode } from "react";
import { AuthWordmark } from "./wordmark";
import "./auth-shell.css";

export function AuthLayout({
  children,
  brandingName,
}: {
  children: ReactNode;
  brandingName?: string | null;
}) {
  return (
    <div className="auth-shell">
      <main className="auth-shell__main">
        <AuthWordmark brandingName={brandingName} />
        {children}
      </main>
    </div>
  );
}

export function AuthPanel({
  title,
  lead,
  children,
}: {
  title: string;
  lead?: string;
  children: ReactNode;
}) {
  return (
    <section className="auth-shell__panel" aria-labelledby="auth-panel-title">
      <header>
        <h1 className="auth-shell__heading" id="auth-panel-title">{title}</h1>
        {lead ? <p className="auth-shell__lead">{lead}</p> : null}
      </header>
      <div className="auth-shell__stack auth-shell__stack--form mt-6">{children}</div>
    </section>
  );
}
