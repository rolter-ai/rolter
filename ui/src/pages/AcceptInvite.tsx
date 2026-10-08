import { ArrowRight, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  acceptInvitation,
  isInvitationSignInRequired,
  previewInvitation,
  type InvitationPreview,
  type InvitationSignInRequired,
} from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useDocumentTitle } from "@/lib/document-title";
import { describeError, type ErrorCopy } from "@/lib/error-copy";
import { roleLabel } from "@/lib/roles";

// the invitee may have no account yet, so this screen renders outside the
// signed-in shell. the token in the url is the only credential it has, and it
// only ever signs in an account the invitation itself creates (#1935)
export default function AcceptInvite({ token }: { token: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { signIn } = useAuth();
  const [invite, setInvite] = useState<InvitationPreview | null>(null);
  const [error, setError] = useState<ErrorCopy | null>(null);
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
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
      setError(describeError(e, t));
    } finally {
      setPending(false);
    }
  };

  // the role by the name the inviter picked it under in the invite dialog, not
  // the id the control plane stores (#2813)
  const role = invite ? roleLabel(t, invite.role) : "";
  const mismatch = confirm.length > 0 && confirm !== pw;
  const ready = invite?.has_account ? !pending : pw.length >= 8 && !mismatch && !pending;

  return (
    <div className="flex min-h-screen items-center justify-center bg-[color:var(--surface-app)] p-4">
      <div className="w-[400px] max-w-full overflow-hidden rounded-xl border bg-background shadow-2xl">
        <div className="vyshivka-rule" />
        <div className="flex flex-col gap-6 p-8">
          <div className="flex items-center gap-3">
            <img src="/logo-mark.svg" alt="" className="h-10 w-10" />
            <span className="font-mono text-[22px] font-semibold tracking-tight">
              rolter<span className="text-[color:var(--red-folk-text)]">.</span>
            </span>
          </div>

          {invite == null ? (
            <p className="text-sm text-muted-foreground">
              {error?.message ?? t("pages.acceptInvite.checking")}
            </p>
          ) : (
            <>
              <div className="flex flex-col gap-1">
                <h1 className="text-xl font-semibold">
                  {t("pages.acceptInvite.title", { org: invite.org_name })}
                </h1>
                {signInReason != null ? (
                  <p role="status" className="text-sm text-muted-foreground">
                    {t(
                      signInReason === "existing_account"
                        ? "pages.acceptInvite.doneExisting"
                        : "pages.acceptInvite.doneSecondFactor",
                      { role, org: invite.org_name },
                    )}
                  </p>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    <Trans
                      i18nKey={
                        invite.has_account
                          ? "pages.acceptInvite.existingIntro"
                          : "pages.acceptInvite.intro"
                      }
                      values={{ email: invite.email, role }}
                      components={{ strong: <strong /> }}
                    />
                  </p>
                )}
              </div>
              {signInReason != null ? (
                // the invitation is spent; what is left is the ordinary sign-in,
                // which the shell shows at any path once there is no session
                <Button
                  type="button"
                  onClick={() => navigate("/", { replace: true })}
                  className="w-full bg-brand-folk text-white hover:bg-brand-press"
                >
                  {t("pages.acceptInvite.toSignIn")} <ArrowRight className="h-4 w-4" />
                </Button>
              ) : (
                <form
                  className="flex flex-col gap-4"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void submit();
                  }}
                >
                  {!invite.has_account && (
                    <>
                      <label className="flex flex-col gap-1.5 text-sm">
                        <span className="text-muted-foreground">
                          {t("pages.acceptInvite.password")}
                        </span>
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
                        <span className="text-muted-foreground">
                          {t("pages.acceptInvite.confirm")}
                        </span>
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
                    </>
                  )}
                  {mismatch && (
                    <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                      {t("pages.acceptInvite.mismatch")}
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
                </form>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
