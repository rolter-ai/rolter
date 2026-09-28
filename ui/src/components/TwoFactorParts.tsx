import { useTranslation } from "react-i18next";

import { QrCode } from "@/components/QrCode";
import { CodeBlock } from "@/components/ui/code-block";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { MfaEnrolment } from "@/lib/api";

/**
 * The pieces of enrolling a second factor that the account screen and the
 * sign-in screen share (#1078, #1852).
 *
 * The account screen shows them in a dialog, the sign-in screen inline in the
 * card, and both used to be free to drift: the QR, the setup key, the field
 * that proves it and the codes it issues have to read the same wherever a
 * user meets them, or the second place looks like a different product.
 *
 * Nothing handled here is ever logged, put in a URL or sent to the UX stream:
 * the secret, the codes and the six digits are all bearer credentials.
 */

/**
 * Scan or type the secret, then prove it with a code.
 *
 * `error` goes on the code field rather than beside it, so the screen reader
 * that announced the field also announces why the code was refused.
 */
export function EnrolSteps({
  enrolment,
  code,
  onCodeChange,
  onSubmit,
  error,
  autoFocus = false,
}: {
  enrolment: MfaEnrolment;
  code: string;
  onCodeChange: (value: string) => void;
  onSubmit: () => void;
  error?: string;
  /** the sign-in card has nothing else to focus; the account dialog does */
  autoFocus?: boolean;
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
          error={error}
        >
          <Input
            value={code}
            // `one-time-code` and not `off`: an authenticator that can fill
            // the field should be allowed to
            autoComplete="one-time-code"
            inputMode="numeric"
            autoFocus={autoFocus}
            maxLength={enrolment.digits}
            onChange={(e) => onCodeChange(e.target.value)}
          />
        </Field>
      </form>
    </div>
  );
}

/**
 * The recovery codes, shown once, and the acknowledgement that has to be
 * ticked before whatever comes next.
 *
 * The acknowledgement is what makes "I have these" a decision rather than a
 * click on whatever was under the pointer; the caller refuses its own primary
 * action until `saved` is true.
 */
export function RecoveryCodesList({
  codes,
  saved,
  onSavedChange,
}: {
  codes: string[];
  saved: boolean;
  onSavedChange: (saved: boolean) => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <CodeBlock value={codes.join("\n")} label={t("account.mfa.codes.label")} />
      <label className="mt-3 flex items-start gap-2 text-sm text-foreground">
        <input
          type="checkbox"
          checked={saved}
          onChange={(e) => onSavedChange(e.target.checked)}
          className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]"
        />
        <span>{t("account.mfa.codes.acknowledge")}</span>
      </label>
    </>
  );
}

/**
 * Save the codes as a text file.
 *
 * A blob URL built in the page, never a link to the server: the codes are in
 * memory here and nothing should put them in a request URL or a log.
 */
export function downloadRecoveryCodes(codes: string[], filename: string) {
  const url = URL.createObjectURL(new Blob([codes.join("\n")], { type: "text/plain" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  URL.revokeObjectURL(url);
}
