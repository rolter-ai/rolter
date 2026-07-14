import { ArrowRight, Eye, EyeOff } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useAuth } from "@/lib/auth";

// login — one of the two sanctioned places the вышивка thread runs
export default function Login() {
  const { signIn } = useAuth();
  const [email, setEmail] = useState("anya@acme.co");
  const [pw, setPw] = useState("correct-horse");
  const [show, setShow] = useState(false);

  return (
    <div className="flex min-h-screen items-center justify-center bg-[color:var(--surface-app)] p-4">
      <div className="w-[400px] max-w-full overflow-hidden rounded-xl border bg-background shadow-2xl">
        <div className="vyshivka-rule" />
        <div className="flex flex-col gap-6 p-8">
          <div className="flex items-center gap-3">
            <img src="/logo-mark.svg" alt="" className="h-10 w-10" />
            <span className="font-mono text-[22px] font-semibold tracking-tight">
              rolter<span className="text-[color:var(--red-folk)]">.</span>
            </span>
          </div>
          <div className="flex flex-col gap-1">
            <h1 className="text-xl font-semibold">Sign in to the control plane</h1>
            <p className="text-sm text-muted-foreground">
              Manage models, keys, routing and usage.
            </p>
          </div>
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              signIn(email.trim() || "operator");
            }}
          >
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">Email</span>
              <Input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">Password</span>
              <span className="relative block">
                <Input
                  type={show ? "text" : "password"}
                  value={pw}
                  onChange={(e) => setPw(e.target.value)}
                  className="pr-10"
                />
                <button
                  type="button"
                  onClick={() => setShow((s) => !s)}
                  aria-label={show ? "Hide password" : "Show password"}
                  className="absolute right-1.5 top-1/2 inline-flex h-7 w-7 -translate-y-1/2 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground"
                >
                  {show ? (
                    <EyeOff className="h-4 w-4" />
                  ) : (
                    <Eye className="h-4 w-4" />
                  )}
                </button>
              </span>
            </label>
            <Button
              type="submit"
              className="w-full bg-brand text-white hover:bg-brand-hover"
            >
              Sign in <ArrowRight className="h-4 w-4" />
            </Button>
          </form>
          <div className="flex flex-col gap-3 border-t border-[color:var(--border-subtle)] pt-5">
            <button
              type="button"
              className="h-9 rounded-md border text-sm text-muted-foreground transition-colors hover:text-foreground"
            >
              Continue with SSO
            </button>
            <span className="text-center text-xs text-[color:var(--text-subtle)]">
              Self-hosted · sessions are local to this deployment.
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
