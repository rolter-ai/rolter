import { ArrowRight, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { AuthCard } from "@/components/ui/auth-card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { previewPasswordReset, redeemPasswordReset, type PasswordResetPreview } from "@/lib/api";
import { useDocumentTitle } from "@/lib/document-title";
import { describeError, type ErrorCopy } from "@/lib/error-copy";

// the member has lost their password, so this renders outside the signed-in
// shell. the token in the url is the only credential it has, and redeeming it
// signs nobody in: the member goes on to the ordinary sign-in, where an armed
// second factor still applies (#2240)
export default function ResetPassword({ token }: { token: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [reset, setReset] = useState<PasswordResetPreview | null>(null);
  const [error, setError] = useState<ErrorCopy | null>(null);
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [pending, setPending] = useState(false);
  const [done, setDone] = useState(false);
  useDocumentTitle(t("pages.resetPassword.title"));

  useEffect(() => {
    let live = true;
    void previewPasswordReset(token)
      .then((p) => live && setReset(p))
      .catch(() => live && setError({ message: t("pages.resetPassword.invalidLink") }));
    return () => {
      live = false;
    };
    // `t` is stable for the lifetime of the loaded catalog
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  const submit = async () => {
    setPending(true);
    setError(null);
    try {
      await redeemPasswordReset(token, pw);
      setDone(true);
    } catch (e) {
      setError(describeError(e, t));
    } finally {
      setPending(false);
    }
  };

  const mismatch = confirm.length > 0 && confirm !== pw;
  const ready = pw.length >= 8 && !mismatch && !pending;

  return (
    <AuthCard
      title={t("pages.resetPassword.title")}
      description={
        reset == null ? (
          (error?.message ?? t("pages.resetPassword.checking"))
        ) : done ? (
          t("pages.resetPassword.done")
        ) : (
          <Trans
            i18nKey="pages.resetPassword.intro"
            values={{ email: reset.email }}
            components={{ strong: <strong /> }}
          />
        )
      }
      descriptionRole={done ? "status" : undefined}
    >
      {reset != null &&
        (done ? (
          <Button
            type="button"
            onClick={() => navigate("/", { replace: true })}
            className="w-full bg-brand-folk text-white hover:bg-brand-press"
          >
            {t("pages.resetPassword.toSignIn")} <ArrowRight className="h-4 w-4" />
          </Button>
        ) : (
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("pages.resetPassword.password")}</span>
              <Input
                type="password"
                name="new-password"
                required
                minLength={8}
                value={pw}
                onChange={(e) => setPw(e.target.value)}
                autoComplete="new-password"
              />
            </label>
            <label className="flex flex-col gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("pages.resetPassword.confirm")}</span>
              <Input
                type="password"
                name="confirm-password"
                required
                aria-invalid={mismatch || undefined}
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                autoComplete="new-password"
              />
            </label>
            {mismatch && (
              <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                {t("pages.resetPassword.mismatch")}
              </p>
            )}
            {error != null && (
              <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                {error.message}
                {error.detail && (
                  <span className="mt-1 block break-words font-mono text-[color:var(--text-subtle)]">
                    {error.detail}
                  </span>
                )}
              </p>
            )}
            <Button
              type="submit"
              disabled={!ready}
              className="w-full bg-brand-folk text-white hover:bg-brand-press"
            >
              {pending ? (
                <>
                  {t("pages.resetPassword.saving")}{" "}
                  <Loader2 className="h-4 w-4 motion-safe:animate-spin" />
                </>
              ) : (
                <>
                  {t("pages.resetPassword.submit")} <ArrowRight className="h-4 w-4" />
                </>
              )}
            </Button>
          </form>
        ))}
    </AuthCard>
  );
}
