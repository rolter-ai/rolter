import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, KeyRound, ShieldCheck } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { QrCode } from "@/components/QrCode";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  beginMfaEnrolment,
  confirmMfaEnrolment,
  disableMfa,
  fetchMfaStatus,
  isOpenModeNoSession,
  regenerateRecoveryCodes,
  type MfaEnrolment,
} from "@/lib/api";
import { useToast } from "@/lib/toast";

/**
 * The account's own second factor (#1078).
 *
 * Self-service throughout: every endpoint behind this panel is `/me/mfa`, so
 * it needs no role and gates on nothing an administrator has to grant. The one
 * thing an administrator controls is the org's `mfa_policy`, and its only
 * effect here is `status.required` — which turns the removal off, because the
 * control plane would refuse it anyway and a button that always 403s is worse
 * than no button.
 *
 * Nothing the panel handles is ever logged, put in a URL or sent to the UX
 * stream: the secret, the codes and the six digits are all bearer credentials,
 * and they travel in request bodies and React state only.
 */
export const MFA_STATUS_KEY = "mfa-status";

export function TwoFactorPanel() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();

  const status = useQuery({
    queryKey: [MFA_STATUS_KEY],
    queryFn: fetchMfaStatus,
    retry: false,
  });

  // what to show once, and only once: the batch a confirm or a regenerate
  // just returned. Held in state and dropped on close — there is no second
  // read path, here or on the server
  const [codes, setCodes] = React.useState<string[] | null>(null);
  const [enrolling, setEnrolling] = React.useState(false);
  const [regenerating, setRegenerating] = React.useState(false);
  const [removing, setRemoving] = React.useState(false);
  const [removeCode, setRemoveCode] = React.useState("");

  const invalidate = () => queryClient.invalidateQueries({ queryKey: [MFA_STATUS_KEY] });

  const regenerate = useMutation({
    mutationFn: regenerateRecoveryCodes,
    onSuccess: (batch) => {
      setRegenerating(false);
      setCodes(batch.recovery_codes);
      void invalidate();
      toast.push({ tone: "success", title: t("account.mfa.toast.regenerated") });
    },
  });

  const remove = useMutation({
    mutationFn: () => disableMfa(removeCode.trim()),
    onSuccess: () => {
      setRemoving(false);
      setRemoveCode("");
      void invalidate();
      toast.push({ tone: "success", title: t("account.mfa.toast.removed") });
    },
  });

  // open mode has no accounts at all, so there is no factor to arm and the
  // keys panel already says why the screen is inert — a second banner saying
  // the same thing twice is noise (#942)
  if (isOpenModeNoSession(status.error)) return null;

  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3">
        <h2 className="text-sm font-medium text-foreground">{t("account.mfa.title")}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{t("account.mfa.subtitle")}</p>
      </header>

      <div className="px-4 py-4">
        {status.isLoading && <PanelSkeleton panels={1} height={96} />}
        {status.error && (
          <LoadError
            error={status.error}
            resource={t("errors.resources.twoFactor")}
            onRetry={() => void status.refetch()}
          />
        )}

        {status.data && !status.data.enabled && (
          <EmptyState
            uxTarget="mfa"
            icon={<ShieldCheck />}
            title={t("account.mfa.off.title")}
            description={
              status.data.required ? t("account.mfa.off.requiredBody") : t("account.mfa.off.body")
            }
            actions={
              <Button
                onClick={() => setEnrolling(true)}
                className="bg-brand-folk text-white hover:bg-brand-press"
              >
                {t("account.mfa.off.cta")}
              </Button>
            }
          />
        )}

        {status.data?.enabled && (
          <div className="flex flex-col gap-4">
            <div className="flex items-start gap-3">
              <ShieldCheck
                aria-hidden
                className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-success-text)]"
              />
              <div className="min-w-0">
                <p className="text-sm font-medium text-foreground">{t("account.mfa.on.title")}</p>
                <p className="mt-1 text-sm text-muted-foreground">
                  {status.data.recovery_codes_remaining === 0
                    ? t("account.mfa.on.noCodes")
                    : t("account.mfa.on.codes", {
                        count: status.data.recovery_codes_remaining,
                      })}
                </p>
                {status.data.required && (
                  <p className="mt-1 text-sm text-[color:var(--status-warning-text)]">
                    {t("account.mfa.on.requiredByOrg")}
                  </p>
                )}
              </div>
            </div>
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  regenerate.reset();
                  setRegenerating(true);
                }}
              >
                <KeyRound className="h-3.5 w-3.5" />
                {t("account.mfa.on.regenerate")}
              </Button>
              <Button
                size="sm"
                variant="destructive"
                // the org made it mandatory, so the control plane answers 403;
                // the title says which, rather than leaving a dead button
                disabled={status.data.required}
                title={status.data.required ? t("account.mfa.on.removeLocked") : undefined}
                onClick={() => {
                  remove.reset();
                  setRemoveCode("");
                  setRemoving(true);
                }}
              >
                {t("account.mfa.on.remove")}
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* mounted only while open, so every enrolment starts from a clean
          dialog: the previous attempt's typed code and its error belong to a
          secret that no longer exists */}
      {enrolling && (
        <EnrolDialog
          onClose={() => setEnrolling(false)}
          onConfirmed={(batch) => {
            setEnrolling(false);
            setCodes(batch);
            void invalidate();
            toast.push({
              tone: "success",
              title: t("account.mfa.toast.enabled"),
            });
          }}
        />
      )}

      <RecoveryCodesDialog codes={codes} onClose={() => setCodes(null)} />

      <ConfirmDialog
        name="mfa-recovery-regenerate"
        open={regenerating}
        onOpenChange={(open) => !open && setRegenerating(false)}
        title={t("account.mfa.confirm.regenerateTitle")}
        description={t("account.mfa.confirm.regenerateBody")}
        confirmLabel={t("account.mfa.confirm.regenerateConfirm")}
        pending={regenerate.isPending}
        error={regenerate.error}
        onConfirm={() => regenerate.mutate()}
      />

      {/* removal asks for a code as well as for consent: the server refuses it
          without one, so a plain confirmation would only ever produce a 400 */}
      <ConfirmDialog
        name="mfa-remove"
        open={removing}
        onOpenChange={(open) => !open && setRemoving(false)}
        title={t("account.mfa.confirm.removeTitle")}
        description={t("account.mfa.confirm.removeBody")}
        confirmLabel={t("account.mfa.confirm.removeConfirm")}
        pending={remove.isPending}
        error={remove.error}
        confirmDisabled={remove.isPending || removeCode.trim().length === 0}
        onConfirm={() => remove.mutate()}
      >
        <Field label={t("account.mfa.confirm.removeCodeLabel")}>
          <Input
            value={removeCode}
            autoComplete="one-time-code"
            inputMode="text"
            onChange={(e) => setRemoveCode(e.target.value)}
          />
        </Field>
      </ConfirmDialog>
    </section>
  );
}

/**
 * Enrolment, in one dialog and two steps: prove the secret, then save the
 * codes the proof issued.
 *
 * The secret is asked for when the dialog opens rather than when the panel
 * loads, because `POST /me/mfa/enroll` *mints* one — asking early would mean a
 * new secret on every visit to the screen, and a half-finished enrolment
 * replaced behind the user's back.
 */
function EnrolDialog({
  onClose,
  onConfirmed,
}: {
  onClose: () => void;
  onConfirmed: (codes: string[]) => void;
}) {
  const { t } = useTranslation();
  const [code, setCode] = React.useState("");

  const enrolment = useQuery({
    queryKey: ["mfa-enrolment"],
    queryFn: beginMfaEnrolment,
    retry: false,
    // a secret is minted per call, so a refetch on window focus — or a cached
    // one handed to the next dialog — would silently replace the QR the user
    // is halfway through scanning
    gcTime: 0,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });

  const confirm = useMutation({
    mutationFn: () => confirmMfaEnrolment(code.trim()),
    onSuccess: (batch) => {
      setCode("");
      onConfirmed(batch.recovery_codes);
    },
  });

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogHeader>
        <DialogTitle>{t("account.mfa.enrol.title")}</DialogTitle>
        <DialogDescription>{t("account.mfa.enrol.subtitle")}</DialogDescription>
      </DialogHeader>

      {enrolment.isLoading && <PanelSkeleton panels={1} height={176} />}
      {enrolment.error && (
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
        />
      )}
      {confirm.isError && (
        <p role="alert" className="mt-3 text-xs text-[color:var(--status-danger-text)]">
          {(confirm.error as Error).message}
        </p>
      )}

      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          disabled={!enrolment.data || confirm.isPending || code.trim().length === 0}
          onClick={() => confirm.mutate()}
        >
          {t("account.mfa.enrol.confirm")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function EnrolSteps({
  enrolment,
  code,
  onCodeChange,
  onSubmit,
}: {
  enrolment: MfaEnrolment;
  code: string;
  onCodeChange: (value: string) => void;
  onSubmit: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col items-center gap-3">
        <QrCode value={enrolment.otpauth_uri} />
        <p className="text-sm text-muted-foreground">{t("account.mfa.enrol.scan")}</p>
      </div>
      {/* the same secret as text, for a desktop authenticator or a phone whose
          camera is not an option. `CodeBlock` owns the copy button */}
      <div className="flex flex-col gap-1.5">
        <p className="text-sm text-muted-foreground">{t("account.mfa.enrol.manual")}</p>
        <CodeBlock value={enrolment.secret} label={t("account.mfa.enrol.secretLabel")} wrap />
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit();
        }}
      >
        <Field
          label={t("account.mfa.enrol.codeLabel")}
          hint={t("account.mfa.enrol.spent", { seconds: enrolment.period })}
        >
          <Input
            value={code}
            // `one-time-code` and not `off`: an authenticator that can fill
            // the field should be allowed to
            autoComplete="one-time-code"
            inputMode="numeric"
            maxLength={enrolment.digits}
            onChange={(e) => onCodeChange(e.target.value)}
          />
        </Field>
      </form>
    </div>
  );
}

/**
 * The codes, shown once.
 *
 * Deliberately not dismissible by the overlay's usual routes alone: the
 * acknowledgement is what makes "I have these" a decision rather than a click
 * on whatever was under the pointer. Escape and the close button still work —
 * trapping someone in a dialog is not a safety feature — but the primary
 * action stays refused until the box is ticked.
 */
function RecoveryCodesDialog({ codes, onClose }: { codes: string[] | null; onClose: () => void }) {
  const { t } = useTranslation();
  const [saved, setSaved] = React.useState(false);

  React.useEffect(() => {
    if (codes) setSaved(false);
  }, [codes]);

  const text = (codes ?? []).join("\n");

  const download = () => {
    // a blob URL built in the page, never a link to the server: the codes are
    // in memory here and nothing should put them in a request URL or a log
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = t("account.mfa.codes.filename");
    link.click();
    URL.revokeObjectURL(url);
  };

  return (
    <Dialog open={!!codes} onOpenChange={(open) => !open && onClose()}>
      <DialogHeader>
        <DialogTitle>{t("account.mfa.codes.title")}</DialogTitle>
        <DialogDescription>{t("account.mfa.codes.body")}</DialogDescription>
      </DialogHeader>
      <CodeBlock value={text} label={t("account.mfa.codes.label")} />
      <label className="mt-3 flex items-start gap-2 text-sm text-foreground">
        <input
          type="checkbox"
          checked={saved}
          onChange={(e) => setSaved(e.target.checked)}
          className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]"
        />
        <span>{t("account.mfa.codes.acknowledge")}</span>
      </label>
      <DialogFooter>
        <Button variant="outline" onClick={download}>
          <Download className="h-4 w-4" />
          {t("account.mfa.codes.download")}
        </Button>
        <Button disabled={!saved} onClick={onClose}>
          {t("account.mfa.codes.done")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
