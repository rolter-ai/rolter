import { ArrowRight, Eye, EyeOff, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { LocalePicker } from "@/components/LocalePicker";
import { FormSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Field } from "@/components/ui/field";
import {
  ApiError,
  getAuthMethods,
  isMfaChallenge,
  login,
  verifyMfaChallenge,
  type AuthMethods,
  type MfaChallenge,
} from "@/lib/api";
import { useAuth } from "@/lib/auth";

/**
 * Guesses one challenge allows, matching `MAX_CHALLENGE_ATTEMPTS` in
 * `crates/rolter-control/src/mfa.rs`.
 *
 * Counted on the client because the control plane deliberately will not say:
 * a wrong code, an expired challenge and an exhausted one are all
 * `invalid_credentials`, so that a guesser cannot tell "keep going" from
 * "start over". A legitimate user does need to be told, and this is the only
 * place that knows.
 */
const MFA_ATTEMPT_BUDGET = 3;

// login — one of the two sanctioned places the вышивка thread runs
export default function Login() {
  const { t } = useTranslation();
  const { signIn, expired } = useAuth();
  // empty by design: the prototype shipped a fake demo account here, which a
  // real deployment then showed to every operator as if it were a login
  const [email, setEmail] = useState("");
  const [pw, setPw] = useState("");
  const [show, setShow] = useState(false);
  const [pending, setPending] = useState(false);
  // what this deployment offers. null while unknown; the permissive shape is
  // assumed on failure so an api hiccup never hides the only way in
  const [methods, setMethods] = useState<AuthMethods | null>(null);
  const [error, setError] = useState<string | null>(null);
  // this control plane serves no auth endpoints at all — it is running in open
  // mode with no store, so `/api/v1/auth/*` is not even mounted. That is the
  // one case the email-only gate exists for, and it is identifiable up front
  // rather than inferred from a failed sign-in (#1160)
  const [openMode, setOpenMode] = useState(false);
  // the password was right and the account owes a factor (#1078). Holding the
  // challenge is what turns this screen into two steps; until #1324 the
  // dashboard modelled only the session branch, so an org that set
  // `mfa_policy` past `off` locked every dashboard user out of login
  const [challenge, setChallenge] = useState<MfaChallenge | null>(null);
  const [code, setCode] = useState("");
  // the control plane spends a challenge after three guesses and answers the
  // same `invalid_credentials` whether the code was wrong or the challenge is
  // already dead — so the budget is counted here, or the user would keep
  // typing codes into a token that can no longer redeem anything
  const [attemptsLeft, setAttemptsLeft] = useState(MFA_ATTEMPT_BUDGET);

  useEffect(() => {
    let live = true;
    void getAuthMethods()
      .then((m) => live && setMethods(m))
      .catch((err) => {
        if (!live) return;
        // a permissive shape either way: an api hiccup must never hide the only
        // way in. what changes is whether we already know the endpoints are gone
        if (err instanceof ApiError && err.status === 404) setOpenMode(true);
        setMethods({ password: true, sso: [] });
      });
    return () => {
      live = false;
    };
  }, []);

  // `methods` is null until /api/v1/auth/methods answers. Treating null as
  // "password is on" flashed the password form on every SSO-only deployment
  // and then swapped it out — the operator saw a field they must not use, and
  // half of them started typing into it (#1180)
  const resolved = methods !== null;
  const showPassword = methods?.password !== false;
  const providers = methods?.sso ?? [];

  // a challenge lives five minutes. Sending the user back when it dies beats
  // letting them finish typing into a token the server will refuse whatever
  // they type, and the reason is said rather than dressed up as a wrong code
  useEffect(() => {
    if (!challenge) return;
    const left = new Date(challenge.expires_at).getTime() - Date.now();
    const timer = setTimeout(
      () => {
        setChallenge(null);
        setError(t("auth.mfa.errors.expired"));
      },
      Math.max(left, 0),
    );
    return () => clearTimeout(timer);
  }, [challenge, t]);

  /**
   * Sign in against a real local account, which is what the self-service
   * `/me/*` endpoints need a session token for.
   *
   * The email-only gate is a fallback for exactly one deployment: open mode
   * with no store, where `/api/v1/auth/login` is not mounted. It used to be
   * the fallback for *every* failure, so a wrong password, a locked account
   * and an SSO-only org all opened the dashboard with no session — the user
   * believed they had signed in, and every later request failed for a reason
   * nothing on screen explained (#1160).
   */
  const submit = async () => {
    const addr = email.trim() || "operator";
    setError(null);
    setPending(true);
    if (openMode) {
      signIn(addr);
      setPending(false);
      return;
    }
    try {
      const res = await login(addr, pw);
      if (isMfaChallenge(res)) {
        setChallenge(res);
        setAttemptsLeft(MFA_ATTEMPT_BUDGET);
        setCode("");
        setPending(false);
        return;
      }
      signIn(res.user.email, res.token, res.user);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        // the endpoint is not served here after all: this is the open-mode
        // deployment, discovered late
        signIn(addr);
      } else {
        setError(loginErrorMessage(err, t));
      }
    } finally {
      setPending(false);
    }
  };

  /**
   * Redeem the challenge for a session.
   *
   * The same field takes a TOTP code or a recovery code — the server tells
   * them apart by shape — so nothing here inspects what was typed. A rejection
   * spends one of the three guesses; the last one takes the challenge with it,
   * and the user goes back to the password step rather than staring at a
   * prompt that can no longer succeed.
   */
  const verify = async () => {
    if (!challenge) return;
    setError(null);
    setPending(true);
    try {
      const res = await verifyMfaChallenge(challenge.mfa_token, code.trim());
      signIn(res.user.email, res.token, res.user);
    } catch (err) {
      const left = attemptsLeft - 1;
      if (err instanceof ApiError && err.status === 401 && left > 0) {
        setAttemptsLeft(left);
        setCode("");
        // the budget is in the message rather than under the field: `Field`
        // shows an error *instead of* its hint, so a count that lived in the
        // hint would be hidden by the very rejection that changed it
        setError(t("auth.mfa.attemptsLeft", { count: left }));
      } else if (err instanceof ApiError && err.status === 401) {
        setChallenge(null);
        setError(t("auth.mfa.errors.spent"));
      } else {
        setError(loginErrorMessage(err, t));
      }
    } finally {
      setPending(false);
    }
  };

  return (
    // a <main>, not a <div>: signed out there is no shell around this, so the
    // card is the whole page and everything on it has to sit inside a landmark
    // — the two rules the story gate now holds this screen to (#1353)
    <main className="flex min-h-screen items-center justify-center bg-[color:var(--surface-app)] p-4">
      <div className="w-[400px] max-w-full overflow-hidden rounded-xl border bg-background shadow-2xl">
        <div className="vyshivka-rule" />
        <div className="flex flex-col gap-6 p-8">
          <div className="flex items-center gap-3">
            <img src="/logo-mark.svg" alt="" className="h-10 w-10" />
            <span className="font-mono text-[22px] font-semibold tracking-tight">
              rolter<span className="text-[color:var(--red-folk-text)]">.</span>
            </span>
          </div>
          <div className="flex flex-col gap-1">
            <h1 className="text-xl font-semibold">{t("auth.title")}</h1>
            <p className="text-sm text-muted-foreground">{t("auth.subtitle")}</p>
          </div>
          {/* the session that was here a moment ago was rejected by the
              control plane (#1196). said once, above the form, so the screen
              explains why it is asking again instead of looking like a
              spontaneous sign-out */}
          {expired && !error && (
            <p
              role="status"
              className="rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-hover)] px-3 py-2 text-sm text-muted-foreground"
            >
              {t("auth.sessionExpired")}
            </p>
          )}
          {!resolved && (
            <>
              <FormSkeleton fields={2} />
              <Skeleton height={36} radius={8} />
            </>
          )}
          {/* one step at a time: the password form and the identity providers
              are gone while a challenge is in flight, because going back is a
              deliberate act (the button below) rather than something to do by
              typing in the field left on screen */}
          {challenge && (
            <form
              className="flex flex-col gap-4"
              onSubmit={(e) => {
                e.preventDefault();
                void verify();
              }}
            >
              <div className="flex flex-col gap-1">
                <h2 className="text-sm font-medium text-foreground">{t("auth.mfa.title")}</h2>
                <p className="text-sm text-muted-foreground">{t("auth.mfa.subtitle")}</p>
              </div>
              <Field
                label={t("auth.mfa.codeLabel")}
                hint={t("auth.mfa.codeHint")}
                error={error ?? undefined}
              >
                <Input
                  name="one-time-code"
                  autoComplete="one-time-code"
                  autoFocus
                  required
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                />
              </Field>
              <Button
                type="submit"
                disabled={pending || code.trim().length === 0}
                className="w-full bg-brand-folk text-white hover:bg-brand-press"
              >
                {pending ? (
                  <>
                    {t("auth.mfa.verifying")} <Loader2 className="h-4 w-4 animate-spin" />
                  </>
                ) : (
                  <>
                    {t("auth.mfa.verify")} <ArrowRight className="h-4 w-4" />
                  </>
                )}
              </Button>
              <button
                type="button"
                onClick={() => {
                  setChallenge(null);
                  setError(null);
                  setCode("");
                }}
                className="text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
              >
                {t("auth.mfa.back")}
              </button>
            </form>
          )}
          {resolved && !challenge && showPassword && (
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
                  name="email"
                  autoComplete="email"
                  autoFocus
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </label>
              <label className="flex flex-col gap-1.5 text-sm">
                <span className="text-muted-foreground">{t("auth.password")}</span>
                <span className="relative block">
                  <Input
                    type={show ? "text" : "password"}
                    name="password"
                    autoComplete="current-password"
                    required
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
                    {show ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                  </button>
                </span>
              </label>
              {error && (
                <p
                  role="alert"
                  className="rounded-md border border-[color:var(--status-danger)]/40 bg-destructive/10 px-3 py-2 text-sm text-[color:var(--status-danger-text)]"
                >
                  {error}
                </p>
              )}
              <Button
                type="submit"
                disabled={pending}
                className="w-full bg-brand-folk text-white hover:bg-brand-press"
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
          {resolved && !challenge && (
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
              <span className="flex items-center justify-center gap-2 text-center text-xs text-[color:var(--text-subtle)]">
                {t("auth.selfHosted")}
                {/* the only language switch lives in the shell's nav, which a
                  signed-out visitor never sees; the menu opens upward, so it
                  sits on the card's last line rather than above the form */}
                <LocalePicker />
              </span>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}

/**
 * One message per reason the control plane can refuse a sign-in. Branching on
 * `code` rather than the message keeps the wording free to be reworded and
 * translated on both sides independently.
 */
function loginErrorMessage(
  err: unknown,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (!(err instanceof ApiError)) {
    // never reached the control plane: a network failure, or it is down
    return t("auth.errors.unavailable");
  }
  switch (err.code) {
    case "invalid_credentials":
      return t("auth.errors.invalidCredentials");
    case "password_login_disabled":
      return t("auth.errors.passwordLoginDisabled");
    case "mfa_enrolment_required":
      // the password was right; the org demands a factor this account has
      // none of. Nothing to retype, and the remedy is an administrator's, so
      // it must not read like a mistyped password (#1078)
      return t("auth.mfa.errors.enrolmentRequired");
    case "too_many_attempts":
      // the lock carries how long it lasts; saying so beats making the user
      // guess, and beats making them poll to find out
      return err.retryAfterSeconds && err.retryAfterSeconds > 0
        ? t("auth.errors.tooManyAttempts_wait", {
            count: Math.ceil(err.retryAfterSeconds),
          })
        : t("auth.errors.tooManyAttempts");
    default:
      return err.status >= 500
        ? t("auth.errors.unavailable")
        : t("auth.errors.unexpected", { message: err.message });
  }
}
