import { RefreshCw } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { ScopeSwitcher } from "@/components/ScopeSwitcher";
import { ChevronDown } from "lucide-react";
import { useScope } from "@/lib/scope";

// per-screen header from the design prototype: title + subtitle on the left,
// scope picker + gateway-healthy pill + refresh on the right, over the
// вышивка rule that recurs under the header on every screen.
export function ScreenHeader({ title, subtitle }: { title: string; subtitle: string }) {
  const queryClient = useQueryClient();
  return (
    <>
      <header className="flex flex-none items-center gap-4 px-[22px] py-4">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
          <p className="mt-0.5 text-sm text-muted-foreground">{subtitle}</p>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <OrgPicker />
          <span className="inline-flex items-center gap-[7px] rounded-full border border-[color:var(--border-subtle)] px-2.5 py-[5px] font-mono text-xs text-muted-foreground">
            <span className="rl-pulse h-[7px] w-[7px] rounded-full bg-[color:var(--status-success)]" />
            gateway&nbsp;healthy
          </span>
          <button
            type="button"
            title="Refresh"
            onClick={() => queryClient.invalidateQueries()}
            className="flex h-[34px] w-[34px] items-center justify-center rounded-md border border-[color:var(--border-subtle)] text-muted-foreground transition-colors hover:bg-[color:var(--surface-hover)] hover:text-foreground"
          >
            <RefreshCw className="h-4 w-4" />
          </button>
        </div>
      </header>
      <div className="vyshivka-rule h-[10px] flex-none opacity-[0.28]" aria-hidden />
    </>
  );
}

// scope picker folded into the header (org · project), dropping the full
// ScopeSwitcher — replaces the old topbar OrgPicker
function OrgPicker() {
  const scope = useScope();
  const [open, setOpen] = React.useState(false);
  const ref = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const orgName = scope.orgs.find((o) => o.id === scope.orgId)?.name;
  const projName = scope.projects.find((p) => p.id === scope.projectId)?.name;
  const summary = orgName ? (projName ? `${orgName} · ${projName}` : orgName) : "Select scope";

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex h-[34px] items-center gap-2 rounded-md border border-[color:var(--border-subtle)] px-2.5 text-sm text-foreground transition-colors hover:border-[color:var(--border-default)]"
      >
        <span className="h-2 w-2 rounded-full bg-[color:var(--red-folk)]" />
        <span className="max-w-[220px] truncate">{summary}</span>
        <ChevronDown className="h-3.5 w-3.5 text-[color:var(--text-subtle)]" />
      </button>
      {open && (
        <div className="absolute right-0 top-[calc(100%+6px)] z-30 w-[300px] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] py-2 shadow-lg">
          <ScopeSwitcher />
        </div>
      )}
    </div>
  );
}
