import { useMutation, useQuery } from "@tanstack/react-query";
import { ArrowRight, Download, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { downloadRecoveryCodes, EnrolSteps, RecoveryCodesList } from "@/components/TwoFactorParts";
import { Button } from "@/components/ui/button";
import {
  ApiError,
  beginSignInEnrolment,
  challengeDeadline,
  confirmSignInEnrolment,
  type EnrolledSignIn,
  type LoginResponse,
  type MfaEnrolmentChallenge,
} from "@/lib/api";

/**
 * Codes one enrolment challenge accepts, matching `MAX_ENROLMENT_ATTEMPTS` in
 * `crates/rolter-control/src/mfa.rs`.
 *
 * Counted here because a wrong code is a 400 that says nothing about how many
 * are left, and the one after the last is a 401 that looks like any other dead
 * token. Without the count, the card says "try the next one" to a member whose
 * next code, right or not, can only send them back to the password step.
 */
const ENROLMENT_ATTEMPT_BUDGET = 5;

/**
 * The sign-in step an org's `required_*` policy sends an unenrolled member
 * through, instead of refusing them (#1852).
 *
 * Two steps inside the sign-in card: prove a freshly minted secret, then save
 * the recovery codes the proof issued. The session exists from the moment the
 * code is accepted, but it is only handed to `onSignedIn` once the codes are
 * acknowledged — the codes are shown exactly once, and the dashboard behind
 * this card is the thing that would make it easy to click past them.
 *
 * Loaded lazily from `Login`: the QR encoder and the code block are only
 * needed by the members a policy binds, so they stay out of the chunk every
 * signed-out visitor downloads (#1709).
 */
export default function SignInEnrolment({
  challenge,
  receivedAt,
  onSignedIn,
  onRestart,
}: {
  challenge: MfaEnrolmentChallenge;
  /**
   * When the challenge arrived, by `Date.now()`. This card is loaded lazily,
   * so its first render can trail the response by a chunk download; the clock
   * starts at the response. Defaults to the first render.
   */
  receivedAt?: number;
  onSignedIn: (session: LoginResponse) => void;
  /**
   * Back to the password step, with the sentence that says why. The token is
   * dead by then — expired, spent, or superseded — and only a new sign-in
   * hands out a live one.
   */
  onRestart: (reason: string | null) => void;
}) {
  const { t } = useTranslation();
  const [code, setCode] = React.useState("");
  const [signedIn, setSignedIn] = React.useState<EnrolledSignIn | null>(null);
  const [saved, setSaved] = React.useState(false);
  const [attemptsLeft, setAttemptsLeft] = React.useState(ENROLMENT_ATTEMPT_BUDGET);
  // fixed once, when the challenge arrived: a later render must not restart
  // the clock, and the server's absolute `expires_at` is never compared with
  // this browser's clock (see challengeDeadline)
  const diesAt = React.useMemo(
    () => challengeDeadline(challenge, receivedAt),
    [challenge, receivedAt],
  );

  const enrolment = useQuery({
    queryKey: ["mfa-sign-in-enrolment", challenge.enrolment_token],
    queryFn: () => beginSignInEnrolment(challenge.enrolment_token),
    retry: false,
    // a secret is minted per call and replaces the pending one, so a refetch
    // on focus would silently swap the QR the user is halfway through scanning
    gcTime: 0,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });

  const confirm = useMutation({
    mutationFn: () => confirmSignInEnrolment(challenge.enrolment_token, code.trim()),
    onSuccess: (result) => {
      setCode("");
      setSignedIn(result);
    },
    onError: (err) => {
      // a wrong code keeps the challenge but spends one of its codes; a dead
      // one never comes back
      if (err instanceof ApiError && err.status === 400) {
        setCode("");
        setAttemptsLeft((left) => left - 1);
      }
    },
  });

  // the last code is spent: the server would refuse the next one whatever it
  // is, so the card goes back now and says why, rather than after one more try
  React.useEffect(() => {
    if (attemptsLeft <= 0) onRestart(t("auth.enrol.errors.spent"));
  }, [attemptsLeft, onRestart, t]);

  // the challenge lives ten minutes. Past that, typing on is pointless, so the
  // card goes back to the password step and says why. Only while enrolling:
  // once the code is accepted the token is spent and the session is real
  React.useEffect(() => {
    if (signedIn) return;
    const left = diesAt - Date.now();
    const timer = setTimeout(() => onRestart(t("auth.enrol.errors.expired")), Math.max(left, 0));
    return () => clearTimeout(timer);
  }, [diesAt, signedIn, onRestart, t]);

  // a token that died on the server — spent, or the account enrolled from
  // somewhere else meanwhile — is the same dead end the timer is, reached by
  // a request instead
  const deadToken = [enrolment.error, confirm.error].some(
    (err) => err instanceof ApiError && err.status === 401,
  );
  React.useEffect(() => {
    if (deadToken) onRestart(t("auth.enrol.errors.restart"));
  }, [deadToken, onRestart, t]);

  if (signedIn) {
    return (
      <div className="flex flex-col gap-4">
        <div className="flex flex-col gap-1">
          <h2 className="text-sm font-medium text-foreground">{t("account.mfa.codes.title")}</h2>
          <p className="text-sm text-muted-foreground">{t("account.mfa.codes.body")}</p>
        </div>
        <div>
          <RecoveryCodesList
            codes={signedIn.recovery_codes}
            saved={saved}
            onSavedChange={setSaved}
          />
        </div>
        <div className="flex flex-col gap-2">
          <Button
            disabled={!saved}
            onClick={() => onSignedIn(signedIn)}
            className="w-full bg-brand-folk text-white hover:bg-brand-press"
          >
            {t("auth.enrol.continue")} <ArrowRight className="h-4 w-4" />
          </Button>
          <Button
            variant="outline"
            onClick={() =>
              downloadRecoveryCodes(signedIn.recovery_codes, t("account.mfa.codes.filename"))
            }
          >
            <Download className="h-4 w-4" />
            {t("account.mfa.codes.download")}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h2 className="text-sm font-medium text-foreground">{t("auth.enrol.title")}</h2>
        <p className="text-sm text-muted-foreground">{t("auth.enrol.subtitle")}</p>
      </div>
      {enrolment.isLoading && <PanelSkeleton panels={1} height={176} />}
      {enrolment.error && !deadToken && (
        <LoadError
          error={enrolment.error}
          resource={t("errors.resources.twoFactorSecret")}
          onRetry={() => void enrolment.refetch()}
        />
      )}
      {enrolment.data && (
        <EnrolSteps
          enrolment={enrolment.data}
          code={code}
          onCodeChange={setCode}
          onSubmit={() => confirm.mutate()}
          error={
            confirm.error && !deadToken
              ? confirmErrorMessage(confirm.error, attemptsLeft, t)
              : undefined
          }
          autoFocus
        />
      )}
      <Button
        disabled={!enrolment.data || confirm.isPending || code.trim().length === 0}
        onClick={() => confirm.mutate()}
        className="w-full bg-brand-folk text-white hover:bg-brand-press"
      >
        {confirm.isPending ? (
          <>
            {t("auth.enrol.confirming")} <Loader2 className="h-4 w-4 motion-safe:animate-spin" />
          </>
        ) : (
          <>
            {t("auth.enrol.confirm")} <ArrowRight className="h-4 w-4" />
          </>
        )}
      </Button>
      <button
        type="button"
        onClick={() => onRestart(null)}
        className="text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
      >
        {t("auth.mfa.back")}
      </button>
    </div>
  );
}

/**
 * The sentence under the code field when a confirm is refused. The control
 * plane's 400 is English prose; the reason it stands for is always the same
 * one here, so it is said in the reader's language instead, with how many of
 * the challenge's codes are left.
 */
function confirmErrorMessage(
  err: unknown,
  attemptsLeft: number,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (!(err instanceof ApiError)) return t("auth.errors.unavailable");
  if (err.status === 400) return t("auth.enrol.errors.attemptsLeft", { count: attemptsLeft });
  return err.status >= 500
    ? t("auth.errors.unavailable")
    : t("auth.errors.unexpected", { message: err.message });
}
