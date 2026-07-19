import { Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { META } from "@/lib/nav";

// branded placeholder for nav areas that don't have a real screen yet —
// mirrors the prototype's stub with the вышивка rule and TODO ribbon
export default function Stub({ screen }: { screen: string }) {
  const [title, subtitle] = META[screen] ?? [screen, ""];
  return (
    <div className="flex min-h-full items-center justify-center p-10">
      <div className="flex max-w-[460px] flex-col items-center gap-[18px] text-center">
        <span className="flex h-[52px] w-[52px] items-center justify-center rounded-[14px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--red-folk)]">
          <Sparkles className="h-5 w-5" />
        </span>
        <div>
          <h2 className="mb-1.5 text-xl font-semibold">{title}</h2>
          <p className="text-sm text-muted-foreground">{subtitle}</p>
        </div>
        <div className="vyshivka-rule h-2 w-[140px] opacity-85" />
        <span className="inline-flex items-center rounded-full bg-[color:var(--status-warning)]/10 px-3 py-[5px] font-mono text-xs text-[color:var(--status-warning)]">
          TODO — we'll come back to this screen
        </span>
        <div className="mt-0.5 flex gap-2.5">
          <Button
            variant="outline"
            onClick={() => window.open("https://github.com/rolter-ai/rolter", "_blank")}
          >
            View docs
          </Button>
        </div>
      </div>
    </div>
  );
}
