import { ArrowRight, Eye, EyeOff, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getAuthMethods, login, type AuthMethods } from "@/lib/api";
import { useAuth } from "@/lib/auth";

// login — one of the two sanctioned places the вышивка thread runs
export default function Login() {
  const { t } = useTranslation();
  const { signIn } = useAuth();
  const [email, setEmail] = useState("anya@acme.co");
  const [pw, setPw] = useState("correct-horse");
  const [show, setShow] = useState(false);
  const [pending, setPending] = useState(false);
  // what this deployment offers. null while unknown; the permissive shape is
  // assumed on failure so an api hiccup never hides the only way in
  const [methods, setMethods] = useState<AuthMethods | null>(null);

  useEffect(() => {
    let live = true;
    void getAuthMethods()
      .then((m) => live && setMethods(m))
      .catch(() => live && setMethods({ password: true, sso: [] }));
    return () => {
      live = false;
    };
  }, []);

  const showPassword = methods?.password !== false;
  const providers = methods?.sso ?? [];

  // try a real local-account login first (needed for the self-service /me/*
  // endpoints, which require a session token). if it fails — wrong creds, or an
  // open-mode deployment with no local accounts — fall back to the email-only
  // gate so the admin dashboard still opens exactly as before.
  const submit = async () => {
    const addr = email.trim() || "operator";
    setPending(true);
    try {
      const res = await login(addr, pw);
      signIn(res.user.email, res.token);
    } catch {
      signIn(addr);
    } finally {
      setPending(false);
    }
  };

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
            <h1 className="text-xl font-semibold">{t("auth.title")}</h1>
            <p className="text-sm text-muted-foreground">{t("auth.subtitle")}</p>
          </div>
          {showPassword && (
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("auth.email")}</span>
              <Input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("auth.password")}</span>
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
                  aria-label={show ? t("auth.hidePassword") : t("auth.showPassword")}
                  className="absolute right-1.5 top-1/2 inline-flex h-7 w-7 -translate-y-1/2 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
              disabled={pending}
              className="w-full bg-brand text-white hover:bg-brand-hover"
            >
              {pending ? (
                <>
                  {t("auth.signingIn")} <Loader2 className="h-4 w-4 animate-spin" />
                </>
              ) : (
                <>
                  {t("auth.signIn")} <ArrowRight className="h-4 w-4" />
                </>
              )}
            </Button>
          </form>
          )}
          <div
            className={
              showPassword
                ? "flex flex-col gap-3 border-t border-[color:var(--border-subtle)] pt-5"
                : "flex flex-col gap-3"
            }
          >
            {/* one button per configured identity provider. a deployment with
                no IdP registered gets none, and never learns sso exists */}
            {providers.map((p) => (
              <a
                key={p.slug}
                href={p.start_url}
                className="inline-flex h-9 items-center justify-center rounded-md border text-sm text-muted-foreground transition-colors hover:text-foreground"
              >
                {t("auth.continueWith", { provider: p.name })}
              </a>
            ))}
            {!showPassword && providers.length === 0 && (
              <span className="text-center text-sm text-muted-foreground">
                {t("auth.noMethod")}
              </span>
            )}
            <span className="text-center text-xs text-[color:var(--text-subtle)]">
              {t("auth.selfHosted")}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
