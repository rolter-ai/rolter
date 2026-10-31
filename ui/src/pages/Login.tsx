import { ArrowRight, Loader2 } from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { LocalePicker } from "@/components/LocalePicker";
import { AuthCard } from "@/components/ui/auth-card";
import { FormSkeleton } from "@/components/LoadingState";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/ui/password-input";
import { Skeleton } from "@/components/ui/skeleton";
import { Field } from "@/components/ui/field";
import {
  ApiError,
  challengeDeadline,
  exchangeSsoCode,
  getAuthMethods,
  isMfaChallenge,
  isMfaEnrolmentChallenge,
  login,
  verifyMfaChallenge,
  type AuthMethods,
  type LoginResponse,
  type MfaChallenge,
  type MfaEnrolmentChallenge,
} from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useDocumentTitle } from "@/lib/document-title";
import { useFormat } from "@/lib/i18n/format";
import { ssoFailureKey } from "@/lib/sso-failures";
import { isOpenMode } from "@/lib/telemetry";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";

/**
 * The enrolment step a `required_*` policy sends an unenrolled member through
 * (#1852). Lazy, because this screen is the first paint for every signed-out
 * visitor and the QR encoder behind it is needed by almost none of them.
 */
const SignInEnrolment = lazy(() => import("@/components/SignInEnrolment"));

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
  const toast = useToast();
  const fmt = useFormat();
  // signed out there is no shell `Screen` to name the tab, so the page does (#2002)
  useDocumentTitle(t("auth.title"));
  // empty by design: a pre-filled demo account here would be shown to every
  // operator of a real deployment as if it were a login
  const [email, setEmail] = useState("");
  const [pw, setPw] = useState("");
  const [pending, setPending] = useState(false);
  // what this deployment offers. null while unknown; the permissive shape is
  // assumed on failure so an api hiccup never hides the only way in
  const [methods, setMethods] = useState<AuthMethods | null>(null);
  // a refusal is the sentence plus, where there is one, what the person can do
  // about it: a self-hosted product has no vendor to call, so the way out is
  // whoever runs this deployment (#2070)
  const [error, setError] = useState<LoginError | null>(null);
  // this control plane serves no auth endpoints at all — it is running in open
  // mode with no store, so `/api/v1/auth/*` is not even mounted. That is the
  // one case the email-only gate exists for, and it is identifiable up front
  // rather than inferred from a failed sign-in (#1160)
  const [openMode, setOpenMode] = useState(false);
  // the same 404 with an admin token configured: no database means no accounts
  // and no sessions, and the analytics and health routes accept the admin token
  // alone. The card takes that token instead of an address, and keeps it as the
  // bearer every request carries (#1909). The control plane says whether it is
  // open in the block it injects into the page, so the two are told apart
  // without a request
  const tokenMode = openMode && !isOpenMode();
  const [adminToken, setAdminToken] = useState("");
  // the password was right and the account owes a factor (#1078). Holding the
  // challenge is what turns this screen into two steps; until #1324 the
  // dashboard modelled only the session branch, so an org that set
  // `mfa_policy` past `off` locked every dashboard user out of login
  const [challenge, setChallenge] = useState<MfaChallenge | null>(null);
  // when that challenge dies, fixed the moment it arrived (see challengeDeadline)
  const [challengeDiesAt, setChallengeDiesAt] = useState(0);
  const [code, setCode] = useState("");
  // the control plane spends a challenge after three guesses and answers the
  // same `invalid_credentials` whether the code was wrong or the challenge is
  // already dead — so the budget is counted here, or the user would keep
  // typing codes into a token that can no longer redeem anything
  const [attemptsLeft, setAttemptsLeft] = useState(MFA_ATTEMPT_BUDGET);
  // the password was right, the org requires a factor, and the account has
  // none (#1852). What used to be a refusal is now a third step: set one up,
  // and the session comes out of that
  const [enrolment, setEnrolment] = useState<MfaEnrolmentChallenge | null>(null);
  const [enrolmentReceivedAt, setEnrolmentReceivedAt] = useState(0);

  /**
   * Hand the session to the shell. An org that announced a requirement ahead
   * of time gets said once, here, on the way in: this is the one moment every
   * member bound by it passes through, and the account screen that can do
   * something about it is a click away (#1852).
   */
  const finishSignIn = useCallback(
    (res: LoginResponse) => {
      if (res.mfa_enrol_by) {
        toast.push({
          tone: "info",
          title: t("auth.enrolBy.title", { date: fmt.date(res.mfa_enrol_by) }),
          detail: t("auth.enrolBy.detail"),
          // a deadline is read, not glanced at
          duration: 15_000,
        });
      }
      signIn(res.user.email, res.token, res.user);
    },
    [fmt, signIn, t, toast],
  );

  // how a browser SSO sign-in ended, read once from the query string the
  // callback redirected to. `slug` is only ever looked up against the methods
  // list, never rendered: the url is not trusted to name anything
  const [ssoNotice, setSsoNotice] = useState<{ key: string; slug: string | null } | null>(null);
  // the exchange code is single use, and React.StrictMode runs this screen's
  // effects twice on mount. A ref survives that simulated remount, so the
  // second pass finds the code already claimed and does nothing; the effect
  // has no cleanup that cancels, so the first pass's answer is still applied
  const ssoHandled = useRef(false);

  useEffect(() => {
    if (ssoHandled.current) return;
    const params = new URLSearchParams(window.location.search);
    const ssoCode = params.get("sso_code");
    const ssoError = params.get("sso_error");
    if (!ssoCode && !ssoError) return;
    ssoHandled.current = true;
    // a code in the address bar outlives the redeeming of it in history
    window.history.replaceState(null, "", window.location.pathname + window.location.hash);
    if (ssoError) {
      setSsoNotice({ key: ssoFailureKey(ssoError), slug: params.get("sso") });
      return;
    }
    setPending(true);
    exchangeSsoCode(ssoCode as string)
      .then((res) => finishSignIn(res))
      .catch((err) => {
        setSsoNotice({
          key:
            err instanceof ApiError && err.code === "invalid_exchange_code"
              ? "auth.sso.exchangeExpired"
              : err instanceof ApiError && err.status < 500
                ? ssoFailureKey("internal_error")
                : "auth.errors.unavailable",
          slug: null,
        });
      })
      .finally(() => setPending(false));
  }, [finishSignIn]);

  const leaveEnrolment = useCallback((reason: string | null) => {
    setEnrolment(null);
    setError(reason ? { message: reason } : null);
  }, []);

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
  const ssoProviderName = providers.find((p) => p.slug === ssoNotice?.slug)?.name;

  // a challenge lives five minutes. Sending the user back when it dies beats
  // letting them finish typing into a token the server will refuse whatever
  // they type, and the reason is said rather than dressed up as a wrong code
  useEffect(() => {
    if (!challenge) return;
    const left = challengeDiesAt - Date.now();
    const timer = setTimeout(
      () => {
        setChallenge(null);
        setError({ message: t("auth.mfa.errors.expired") });
      },
      Math.max(left, 0),
    );
    return () => clearTimeout(timer);
  }, [challenge, challengeDiesAt, t]);

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
    if (tokenMode) {
      // nothing to verify against: there is no endpoint that checks it. A wrong
      // token is refused by the first screen that uses it, with an explanation
      // and a way back here (LoadError, `adminToken`)
      signIn("admin", adminToken.trim());
      setPending(false);
      return;
    }
    if (openMode) {
      signIn(addr);
      setPending(false);
      return;
    }
    try {
      const res = await login(addr, pw);
      if (isMfaChallenge(res)) {
        setChallengeDiesAt(challengeDeadline(res));
        setChallenge(res);
        setAttemptsLeft(MFA_ATTEMPT_BUDGET);
        setCode("");
        setPending(false);
        return;
      }
      if (isMfaEnrolmentChallenge(res)) {
        setEnrolmentReceivedAt(Date.now());
        setEnrolment(res);
        setPending(false);
        return;
      }
      finishSignIn(res);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        // the endpoint is not served here after all: this is the open-mode
        // deployment, discovered late
        signIn(addr);
      } else {
        setError(loginError(err, t, providers.length > 0));
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
      finishSignIn(res);
    } catch (err) {
      const left = attemptsLeft - 1;
      if (err instanceof ApiError && err.status === 401 && left > 0) {
        setAttemptsLeft(left);
        setCode("");
        // the budget is in the message rather than under the field: `Field`
        // shows an error *instead of* its hint, so a count that lived in the
        // hint would be hidden by the very rejection that changed it
        setError({
          message: t("auth.mfa.attemptsLeft", { count: left }),
          help: t("auth.mfa.stuckHelp"),
        });
      } else if (err instanceof ApiError && err.status === 401) {
        setChallenge(null);
        setError({ message: t("auth.mfa.errors.spent") });
      } else {
        setError(loginError(err, t, providers.length > 0));
      }
    } finally {
      setPending(false);
    }
  };

  const errorId = "login-error";

  return (
    // AuthCard owns the <main> landmark and the h1: signed out there is no
    // shell around this, so the card is the whole page (#1353, #2228)
    <AuthCard title={t("auth.title")} description={t("auth.subtitle")}>
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
      {ssoNotice && (
        <div
          role="alert"
          className="flex flex-col gap-1 rounded-md border border-[color:var(--status-danger)]/40 bg-destructive/10 px-3 py-2 text-sm text-[color:var(--status-danger-text)]"
        >
          <p>{t(ssoNotice.key)}</p>
          {ssoProviderName && <p>{t("auth.sso.provider", { name: ssoProviderName })}</p>}
        </div>
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
            error={error ? [error.message, error.help].filter(Boolean).join(" ") : undefined}
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
            variant="brand"
            type="submit"
            disabled={pending || code.trim().length === 0}
            className="w-full"
          >
            {pending ? (
              <>
                {t("auth.mfa.verifying")} <Loader2 className="h-4 w-4 motion-safe:animate-spin" />
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
      {enrolment && (
        <Suspense fallback={<FormSkeleton fields={2} />}>
          <SignInEnrolment
            challenge={enrolment}
            receivedAt={enrolmentReceivedAt}
            onSignedIn={finishSignIn}
            onRestart={leaveEnrolment}
          />
        </Suspense>
      )}
      {resolved && !challenge && !enrolment && showPassword && (
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          {tokenMode ? (
            <Field label={t("auth.adminToken.label")} hint={t("auth.adminToken.hint")}>
              <PasswordInput
                name="admin-token"
                autoComplete="off"
                autoFocus
                required
                value={adminToken}
                onChange={(e) => setAdminToken(e.target.value)}
              />
            </Field>
          ) : (
            <>
              <Field label={t("auth.email")}>
                <Input
                  type="email"
                  name="email"
                  autoComplete="email"
                  autoFocus
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? errorId : undefined}
                />
              </Field>
              <Field label={t("auth.password")}>
                <PasswordInput
                  name="password"
                  autoComplete="current-password"
                  required
                  value={pw}
                  onChange={(e) => setPw(e.target.value)}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? errorId : undefined}
                />
              </Field>
            </>
          )}
          {error && (
            <div
              id={errorId}
              role="alert"
              className="flex flex-col gap-1 rounded-md border border-[color:var(--status-danger)]/40 bg-destructive/10 px-3 py-2 text-sm text-[color:var(--status-danger-text)]"
            >
              <p>{error.message}</p>
              {error.help && <p className="text-xs">{error.help}</p>}
            </div>
          )}
          <Button
            variant="brand"
            type="submit"
            disabled={pending || (tokenMode && adminToken.trim().length === 0)}
            className="w-full"
          >
            {pending ? (
              <>
                {t("auth.signingIn")} <Loader2 className="h-4 w-4 motion-safe:animate-spin" />
              </>
            ) : (
              <>
                {tokenMode ? t("auth.adminToken.submit") : t("auth.signIn")}{" "}
                <ArrowRight className="h-4 w-4" />
              </>
            )}
          </Button>
        </form>
      )}
      {resolved && !challenge && !enrolment && (
        <div
          className={
            showPassword
              ? "flex flex-col gap-3 border-t border-[color:var(--border-subtle)] pt-5"
              : "flex flex-col gap-3"
          }
        >
          {/* one button per configured identity provider. a deployment with
                no IdP registered gets none, and never learns sso exists */}
          {providers.map((p, i) => (
            <a
              key={p.slug}
              href={p.start_url}
              // with no password form, signing in is the one action this screen
              // exists for, so the first provider is the brand button (DESIGN.md)
              className={cn(
                buttonVariants({ variant: !showPassword && i === 0 ? "brand" : "outline" }),
                "w-full",
                (showPassword || i !== 0) && "text-muted-foreground hover:text-foreground",
              )}
            >
              {t("auth.continueWith", { provider: p.name })}
            </a>
          ))}
          {!showPassword && providers.length === 0 && (
            <span className="text-center text-sm text-muted-foreground">{t("auth.noMethod")}</span>
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
    </AuthCard>
  );
}

/** a refusal: what happened, and what the person can do about it (#2070) */
interface LoginError {
  message: string;
  help?: string;
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

/**
 * One message per reason the control plane can refuse a sign-in. Branching on
 * `code` rather than the message keeps the wording free to be reworded and
 * translated on both sides independently.
 *
 * `hasSso` is whether a provider button is on screen: "use single sign-on
 * below" points at nothing on a deployment with none.
 */
function loginError(err: unknown, t: Translate, hasSso: boolean): LoginError {
  if (!(err instanceof ApiError)) {
    // never reached the control plane: a network failure, or it is down
    return { message: t("auth.errors.unavailable") };
  }
  switch (err.code) {
    case "invalid_credentials":
      return { message: t("auth.errors.invalidCredentials"), help: t("auth.errors.resetHelp") };
    case "password_login_disabled":
      return {
        message: t(
          hasSso ? "auth.errors.passwordLoginDisabled" : "auth.errors.passwordLoginDisabledNoSso",
        ),
      };
    case "mfa_enrolment_required":
      // the password was right and the org demands a factor this account has
      // none of — and this control plane cannot enrol one, since it has no key
      // to seal the secret with. Any other deployment walks the user through
      // enrolment instead (#1852). Nothing to retype, and the remedy is an
      // operator's, so it must not read like a mistyped password (#1078)
      return { message: t("auth.mfa.errors.enrolmentRequired") };
    case "too_many_attempts":
      // the lock carries how long it lasts; saying so beats making the user
      // guess, and beats making them poll to find out. waiting only helps a
      // typo, so the way out for a forgotten password is said as well
      return {
        message:
          err.retryAfterSeconds && err.retryAfterSeconds > 0
            ? t("auth.errors.tooManyAttempts_wait", { count: Math.ceil(err.retryAfterSeconds) })
            : t("auth.errors.tooManyAttempts"),
        help: t("auth.errors.lockedHelp"),
      };
    default:
      return {
        message:
          err.status >= 500
            ? t("auth.errors.unavailable")
            : t("auth.errors.unexpected", { message: err.message }),
      };
  }
}
