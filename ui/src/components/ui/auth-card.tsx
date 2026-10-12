import * as React from "react";

// the card a signed-out visitor sees: wordmark, вышивка rule, heading, then
// whatever the screen asks. signed out there is no shell, so the card is the
// whole page and owns the `<main>` landmark and the `h1` — the two things
// AcceptInvite lost when it carried its own copy (#2069, #2228)
export interface AuthCardProps {
  title: React.ReactNode;
  /** the line under the title; pass `descriptionRole="status"` when it reports an outcome */
  description?: React.ReactNode;
  descriptionRole?: "status";
  children?: React.ReactNode;
}

export function AuthCard({ title, description, descriptionRole, children }: AuthCardProps) {
  return (
    <main className="flex min-h-screen items-center justify-center bg-[color:var(--surface-app)] p-4">
      <div className="w-[400px] max-w-full overflow-hidden rounded-xl border bg-background shadow-2xl">
        <div className="vyshivka-rule" aria-hidden />
        <div className="flex flex-col gap-6 p-8">
          <div className="flex items-center gap-3">
            <img src="/logo-mark.svg" alt="" className="h-10 w-10" />
            <span className="font-mono text-[22px] font-semibold tracking-tight">
              rolter<span className="text-[color:var(--red-folk-text)]">.</span>
            </span>
          </div>
          <div className="flex flex-col gap-1">
            <h1 className="text-xl font-semibold">{title}</h1>
            {description != null && (
              <p role={descriptionRole} className="text-sm text-muted-foreground">
                {description}
              </p>
            )}
          </div>
          {children}
        </div>
      </div>
    </main>
  );
}
