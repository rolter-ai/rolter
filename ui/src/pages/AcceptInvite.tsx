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
  acceptInvitation,
  ApiError,
  isInvitationSignInRequired,
  previewInvitation,
  type InvitationPreview,
  type InvitationSignInRequired,
} from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useDocumentTitle } from "@/lib/document-title";
import { describeError, type ErrorCopy } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { roleLabel } from "@/lib/roles";

// the invitee may have no account yet, so this screen renders outside the
// signed-in shell. the token in the url is the only credential it has, and it
// only ever signs in an account the invitation itself creates (#1935)
export default function AcceptInvite({ token }: { token: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { signIn } = useAuth();
  const fmt = useFormat();
  const [invite, setInvite] = useState<InvitationPreview | null>(null);
  const [error, setError] = useState<ErrorCopy | null>(null);
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
  // the rules are said once the person has moved on from a field (or, for the
  // confirmation, typed as much as the password), not on the first keystroke
  const [pwBlurred, setPwBlurred] = useState(false);
  const [confirmBlurred, setConfirmBlurred] = useState(false);
  // a refusal retrying cannot fix: the button stays off and the way on is the
  // sign-in page (#2067)
  const [refused, setRefused] = useState(false);
  const [pending, setPending] = useState(false);
  // accepted, but the invitee has to sign in for a session: an existing
  // account, or a new one its org requires a second factor from
  const [signInReason, setSignInRequired] = useState<InvitationSignInRequired["reason"] | null>(
    null,
  );
  // outside the shell, so the page names the tab itself (#2002); the org is
  // only known once the preview answers, and a dead link never names one
  useDocumentTitle(
    invite
      ? t("pages.acceptInvite.title", { org: invite.org_name })
      : t("pages.acceptInvite.genericTitle"),
  );

  useEffect(() => {
    let live = true;
    void previewInvitation(token)
      .then((p) => live && setInvite(p))
      .catch(() => live && setError({ message: t("pages.acceptInvite.invalidLink") }));
    return () => {
      live = false;
    };
    // `t` is stable for the lifetime of the loaded catalog
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  const submit = async () => {
    if (!invite?.has_account && (pw.length < 8 || confirm !== pw)) {
      setPwBlurred(true);
      setConfirmBlurred(true);
      return;
    }
    setPending(true);
    setError(null);
    try {
      // an existing account keeps its own password, so none is sent for it
      const res = await acceptInvitation(token, invite?.has_account ? undefined : pw);
      if (isInvitationSignInRequired(res)) {
        setSignInRequired(res.reason);
        return;
      }
      signIn(res.user.email, res.token);
      // land on the dashboard rather than back on a spent link. this has to
      // go through the router: a bare history.replaceState changed the url bar
      // but not the router's location, so the shell re-rendered this screen
      // against a token that had just been consumed
      navigate("/dashboard", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        // used, revoked or expired between opening the link and submitting it
        setRefused(true);
        setError({ message: t("pages.acceptInvite.errors.spent") });
      } else if (e instanceof ApiError && e.status === 403) {
        setRefused(true);
        setError({ message: t("pages.acceptInvite.errors.deactivated") });
      } else if (e instanceof ApiError && e.status === 429 && e.retryAfterSeconds) {
        setError({
          message: t("pages.acceptInvite.errors.wait", { count: Math.ceil(e.retryAfterSeconds) }),
        });
      } else {
        setError(describeError(e, t));
      }
    } finally {
      setPending(false);
    }
  };

  // the role by the name the inviter picked it under in the invite dialog, not
  // the id the control plane stores (#2813)
  const role = invite ? roleLabel(t, invite.role) : "";
  const tooShort = pw.length < 8;
  const mismatch =
    confirm !== pw && (confirmBlurred || (confirm.length > 0 && confirm.length >= pw.length));
  const grants = invite
    ? t(
        invite.scope_type === "project"
          ? "pages.acceptInvite.grantsProject"
          : invite.scope_type === "team"
            ? "pages.acceptInvite.grantsTeam"
            : "pages.acceptInvite.grantsOrg",
        { role, org: invite.org_name, scope: invite.scope_name },
      )
    : "";
  const toSignIn = () => navigate("/", { replace: true });

  return (
    <AuthCard
      title={
        invite == null
          ? t("pages.acceptInvite.fallbackTitle")
          : t("pages.acceptInvite.title", { org: invite.org_name })
      }
      description={
        invite == null ? (
          (error?.message ?? t("pages.acceptInvite.checking"))
        ) : signInReason != null ? (
          t(
            signInReason === "existing_account"
              ? "pages.acceptInvite.doneExisting"
              : "pages.acceptInvite.doneSecondFactor",
            { role, org: invite.org_name },
          )
        ) : (
          <Trans
            i18nKey={
              invite.has_account ? "pages.acceptInvite.existingIntro" : "pages.acceptInvite.intro"
            }
            values={{ email: invite.email, role }}
            components={{ strong: <strong /> }}
          />
        )
      }
      descriptionRole={signInReason != null ? "status" : undefined}
    >
      {invite == null && error != null && (
        <Button variant="brand" type="button" onClick={toSignIn} className="w-full">
          {t("pages.acceptInvite.signIn")} <ArrowRight className="h-4 w-4" />
        </Button>
      )}
      {invite != null &&
        (signInReason != null ? (
          // the invitation is spent; what is left is the ordinary sign-in,
          // which the shell shows at any path once there is no session
          <Button variant="brand" type="button" onClick={toSignIn} className="w-full">
            {t("pages.acceptInvite.toSignIn")} <ArrowRight className="h-4 w-4" />
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
            <p className="text-sm text-muted-foreground">
              {grants} {t("pages.acceptInvite.expires", { date: fmt.dateTime(invite.expires_at) })}
            </p>
            {!invite.has_account && (
              <>
                <Field
                  label={t("pages.acceptInvite.password")}
                  hint={t("pages.acceptInvite.passwordHint")}
                  error={
                    pwBlurred && tooShort ? t("pages.acceptInvite.passwordTooShort") : undefined
                  }
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
                  label={t("pages.acceptInvite.confirm")}
                  error={mismatch ? t("pages.acceptInvite.mismatch") : undefined}
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
              </>
            )}
            {error != null && (
              <div className="flex flex-col gap-1">
                <FieldError id="accept-error" error={error.message} />
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
                  {t(
                    invite.has_account
                      ? "pages.acceptInvite.accepting"
                      : "pages.acceptInvite.creating",
                  )}{" "}
                  <Loader2 className="h-4 w-4 motion-safe:animate-spin" />
                </>
              ) : (
                <>
                  {t("pages.acceptInvite.accept")} <ArrowRight className="h-4 w-4" />
                </>
              )}
            </Button>
            {refused && (
              <Button type="button" variant="outline" onClick={toSignIn} className="w-full">
                {t("pages.acceptInvite.signIn")}
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
