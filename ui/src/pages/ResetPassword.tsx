import { ArrowRight, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { AuthCard } from "@/components/ui/auth-card";
import { LocalePicker } from "@/components/LocalePicker";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { FieldError } from "@/components/ui/field-error";
import { PasswordInput } from "@/components/ui/password-input";
import {
  ApiError,
  previewPasswordReset,
  redeemPasswordReset,
  type PasswordResetPreview,
} from "@/lib/api";
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
  // the rules are said once the person has moved on from a field (or, for the
  // confirmation, typed as much as the password), not on the first keystroke
  const [pwBlurred, setPwBlurred] = useState(false);
  const [confirmBlurred, setConfirmBlurred] = useState(false);
  // a link that was spent or expired between the preview and the submit:
  // retrying cannot fix it, so the way on is the sign-in page
  const [refused, setRefused] = useState(false);
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
    if (pw.length < 8 || confirm !== pw) {
      setPwBlurred(true);
      setConfirmBlurred(true);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await redeemPasswordReset(token, pw);
      setDone(true);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        setRefused(true);
        setError({ message: t("pages.resetPassword.invalidLink") });
      } else {
        setError(describeError(e, t));
      }
    } finally {
      setPending(false);
    }
  };

  const tooShort = pw.length < 8;
  const mismatch =
    confirm !== pw && (confirmBlurred || (confirm.length > 0 && confirm.length >= pw.length));
  const toSignIn = () => navigate("/", { replace: true });

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
          <Button variant="brand" type="button" onClick={toSignIn} className="w-full">
            {t("pages.resetPassword.toSignIn")} <ArrowRight className="h-4 w-4" />
          </Button>
        ) : (
          <form
            noValidate
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <Field
              label={t("pages.resetPassword.password")}
              hint={t("pages.resetPassword.passwordHint")}
              error={pwBlurred && tooShort ? t("pages.resetPassword.passwordTooShort") : undefined}
            >
              <PasswordInput
                name="new-password"
                required
                value={pw}
                onChange={(e) => setPw(e.target.value)}
                onBlur={() => setPwBlurred(true)}
                autoComplete="new-password"
              />
            </Field>
            <Field
              label={t("pages.resetPassword.confirm")}
              error={mismatch ? t("pages.resetPassword.mismatch") : undefined}
            >
              <PasswordInput
                name="confirm-password"
                required
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                onBlur={() => setConfirmBlurred(true)}
                autoComplete="new-password"
              />
            </Field>
            {error != null && (
              <div className="flex flex-col gap-1">
                <FieldError id="reset-error" error={error.message} />
                {error.detail && (
                  <span className="break-words font-mono text-xs text-[color:var(--text-subtle)]">
                    {error.detail}
                  </span>
                )}
              </div>
            )}
            <Button variant="brand" type="submit" disabled={pending || refused} className="w-full">
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
            {refused && (
              <Button type="button" variant="outline" onClick={toSignIn} className="w-full">
                {t("pages.resetPassword.toSignIn")}
              </Button>
            )}
          </form>
        ))}
      <div className="flex justify-center">
        <LocalePicker />
      </div>
    </AuthCard>
  );
}
