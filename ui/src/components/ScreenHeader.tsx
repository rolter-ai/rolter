import { RefreshCw } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";

// per-screen header from the design prototype: title + subtitle on the left,
// gateway-healthy pill + refresh on the right, over the вышивка rule that
// recurs under the header on every screen. the org/project scope picker lives
// in the sidebar user menu, not here.
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
