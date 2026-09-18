import { ArrowRight, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { acceptInvitation, previewInvitation, type InvitationPreview } from "@/lib/api";
import { useAuth } from "@/lib/auth";

// the invitee has no account yet, so this screen renders outside the signed-in
// shell. the token in the url is the only credential it has.
export default function AcceptInvite({ token }: { token: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { signIn } = useAuth();
  const [invite, setInvite] = useState<InvitationPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [pending, setPending] = useState(false);

  useEffect(() => {
    let live = true;
    void previewInvitation(token)
      .then((p) => live && setInvite(p))
      .catch(() => live && setError(t("pages.acceptInvite.invalidLink")));
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
      const res = await acceptInvitation(token, pw);
      signIn(res.user.email, res.token);
      // land on the dashboard rather than back on a spent link. this has to
      // go through the router: a bare history.replaceState changed the url bar
      // but not the router's location, so the shell re-rendered this screen
      // against a token that had just been consumed
      navigate("/dashboard", { replace: true });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setPending(false);
    }
  };

  const mismatch = confirm.length > 0 && confirm !== pw;
  const ready = pw.length >= 8 && !mismatch && !pending;

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
              {error ?? t("pages.acceptInvite.checking")}
            </p>
          ) : (
            <>
              <div className="flex flex-col gap-1">
                <h1 className="text-xl font-semibold">
                  {t("pages.acceptInvite.title", { org: invite.org_name })}
                </h1>
                <p className="text-sm text-muted-foreground">
                  <Trans
                    i18nKey="pages.acceptInvite.intro"
                    values={{ email: invite.email, role: invite.role }}
                    components={{ strong: <strong /> }}
                  />
                </p>
              </div>
              <form
                className="flex flex-col gap-4"
                onSubmit={(e) => {
                  e.preventDefault();
                  void submit();
                }}
              >
                <label className="flex flex-col gap-1.5 text-sm">
                  <span className="text-muted-foreground">{t("pages.acceptInvite.password")}</span>
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
                  <span className="text-muted-foreground">{t("pages.acceptInvite.confirm")}</span>
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
                    {t("pages.acceptInvite.mismatch")}
                  </p>
                )}
                {error != null && (
                  <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                    {error}
                  </p>
                )}
                <Button
                  type="submit"
                  disabled={!ready}
                  className="w-full bg-brand-folk text-white hover:bg-brand-press"
                >
                  {pending ? (
                    <>
                      {t("pages.acceptInvite.creating")}{" "}
                      <Loader2 className="h-4 w-4 animate-spin" />
                    </>
                  ) : (
                    <>
                      {t("pages.acceptInvite.accept")} <ArrowRight className="h-4 w-4" />
                    </>
                  )}
                </Button>
              </form>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
