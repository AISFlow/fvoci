// Adapted from fvoci/FVOCI apps/web/src/features/auth/auth-layout.tsx
import type { ReactNode } from "react";
import { type OperatorInfo, ServiceInfoFooter } from "@/features/legal/operator-info";
import { AuthWordmark } from "./wordmark";
import "./auth-shell.css";

type AuthWidth = "narrow" | "wide";

export function AuthLayout({
  children,
  brandingName,
  footerOperator,
  width = "narrow",
  showWordmark = true,
}: {
  children: ReactNode;
  brandingName?: string | null;
  footerOperator?: OperatorInfo | null;
  width?: AuthWidth;
  showWordmark?: boolean;
}) {
  return (
    <div className="auth-shell">
      <main
        className={
          width === "wide" ? "auth-shell__main auth-shell__main--wide" : "auth-shell__main"
        }
      >
        {showWordmark ? <AuthWordmark brandingName={brandingName} /> : null}
        {children}
        {footerOperator !== undefined ? <ServiceInfoFooter operator={footerOperator} /> : null}
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
