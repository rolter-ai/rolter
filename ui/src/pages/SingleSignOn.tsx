import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Eraser,
  KeyRound,
  Loader2,
  Pencil,
  Plus,
  ShieldCheck,
  Trash2,
  Users,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { GroupMappings, MAPPABLE_ROLES, roleLabel } from "@/components/GroupMappings";
import { LoadError } from "@/components/LoadError";
import { ListSummary, PageBody, Pill, RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  createSsoGroupMapping,
  createSsoProvider,
  updateSsoProvider,
  deleteSsoGroupMapping,
  deleteSsoProvider,
  fetchAuthPolicy,
  fetchMemberships,
  fetchSsoGroupMappings,
  fetchSsoProviders,
  ssoRedirectUri,
  updateAuthPolicy,
  MFA_POLICIES,
  type MfaPolicy,
  type OrgAuthPolicy,
  type PublicUrl,
  type SsoProviderRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import {
  distinctPeople,
  locksOutMembers,
  locksOutSsoMembers,
  secretGap,
  type SecretGap,
} from "@/lib/sso-lockout";
import { SSO_SLUG_MAX, ssoSlugProblem, suggestSsoSlug } from "@/lib/sso-slug";
import { errorDetail, useToast } from "@/lib/toast";
import { usePublicUrl } from "@/lib/use-public-url";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PROVIDERS_KEY = "sso-providers";
const POLICY_KEY = "org-auth-policy";
const MAPPINGS_KEY = "sso-group-mappings";

// a labelled line inside a provider card: mono value, optionally copyable. an
// address wraps instead of truncating, because the end of it is what tells the
// redirect uri from the login url; `note` says what the value is for
function Detail({
  label,
  value,
  copyLabel,
  note,
  wrap = false,
}: {
  label: string;
  value: string;
  copyLabel?: string;
  note?: string;
  wrap?: boolean;
}) {
  return (
    <div className="flex min-w-0 items-start gap-2">
      <span className="w-[104px] flex-none text-[0.6875rem] uppercase leading-4 tracking-[0.07em] text-[color:var(--text-subtle)]">
        {label}
      </span>
      <div className="min-w-0 flex-1">
        <span
          className={cn(
            "block font-mono text-xs leading-4 text-[color:var(--text-secondary)]",
            wrap ? "break-all" : "truncate",
          )}
        >
          {value}
        </span>
        {note && <span className="mt-0.5 block text-xs text-muted-foreground">{note}</span>}
      </div>
      {/* centred on the value's first line, so a copyable row keeps the same
          rhythm as the rows around it */}
      {copyLabel && <CopyButton value={value} label={copyLabel} className="-my-2" />}
    </div>
  );
}

/**
 * A warning with a title and the lines that explain it: the screen's one
 * callout shape, for the notice above the list and for the ones a confirmation
 * carries.
 */
function WarningNote({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div
      role="note"
      className="flex items-start gap-2.5 rounded-lg border border-[color:var(--status-warning)]/30 bg-[color:var(--status-warning)]/5 px-4 py-3"
    >
      <AlertTriangle
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-warning-text)]"
      />
      <div className="min-w-0 space-y-1 text-sm">
        <p className="font-medium text-foreground">{title}</p>
        {children}
      </div>
    </div>
  );
}

/**
 * Said once above the provider list when `ROLTER_PUBLIC_URL` is unset (#2083).
 *
 * Every URL on this screen is built from the control plane's public base, and
 * without the variable that base is the built-in default. A redirect URI
 * copied from here into an identity provider would then send any user whose
 * browser is not on the control plane's own host to an address that is not
 * rolter, and the provider's error would name a mismatch rather than the cause.
 */
function PublicUrlNotice({ publicUrl }: { publicUrl: PublicUrl }) {
  const { t } = useTranslation();
  return (
    <WarningNote title={t("pages.sso.publicUrl.title")}>
      <p className="text-muted-foreground">
        <Trans
          i18nKey="pages.sso.publicUrl.body"
          values={{ url: publicUrl.public_url }}
          components={{ code: <code className="font-mono text-xs text-foreground" /> }}
        />
      </p>
    </WarningNote>
  );
}

/**
 * What a change to a provider would leave nobody able to do (#2084).
 *
 * Raised inside the confirmation for taking the last enabled provider out of
 * service or deleting it while password sign-in is off. It says who still gets
 * in, and only from what the control plane enforces: superadmins are exempt
 * from `allow_password_login = false` (`auth_policy.rs`), and an account that
 * signed up through a provider was created with no password, so turning
 * password sign-in back on does not bring it back.
 */
function LockoutNotice({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <WarningNote title={t("pages.sso.lockout.title")}>
      <p className="text-muted-foreground">{t("pages.sso.lockout.body", { name })}</p>
      <p className="text-muted-foreground">{t("pages.sso.lockout.noPassword")}</p>
    </WarningNote>
  );
}

/**
 * Who turning single sign-on off shuts out, and who still gets in (#2326).
 *
 * While the switch is off the callback refuses every provider of the org, and
 * an account a provider created has no password, so password sign-in being on
 * does not bring those members back. An account that holds a password, such as
 * one made from an invitation, signs in as before. The superadmin exemption
 * is from the password switch, not this one, so it is not claimed here.
 */
function SsoOffNotice() {
  const { t } = useTranslation();
  return (
    <WarningNote title={t("pages.sso.policy.ssoConfirm.notice.title")}>
      <p className="text-muted-foreground">{t("pages.sso.policy.ssoConfirm.notice.noPassword")}</p>
      <p className="text-muted-foreground">{t("pages.sso.policy.ssoConfirm.notice.stillIn")}</p>
    </WarningNote>
  );
}

/**
 * The enabled providers that carry the "No client secret" badge, named inside
 * the confirmation for turning password sign-in off (#2084).
 *
 * The control plane refuses that change when no provider is enabled, but not
 * when the only one that is cannot finish a token exchange. A provider with no
 * secret is legitimate for a public client, so the copy states the condition
 * rather than the failure.
 */
function NoSecretNotice({ gap }: { gap: SecretGap }) {
  const { t } = useTranslation();
  return (
    <WarningNote
      title={t("pages.sso.policy.passwordConfirm.noSecret.title", { count: gap.missing.length })}
    >
      <ul className="flex flex-col gap-0.5">
        {gap.missing.map((provider) => (
          <li key={provider.id} className="flex flex-wrap items-baseline gap-x-2 text-foreground">
            <span>{provider.name}</span>
            <code className="font-mono text-xs text-muted-foreground">{provider.slug}</code>
          </li>
        ))}
      </ul>
      <p className="text-muted-foreground">{t("pages.sso.policy.passwordConfirm.noSecret.body")}</p>
      {gap.all && (
        <p className="text-muted-foreground">
          {t("pages.sso.policy.passwordConfirm.noSecret.all")}
        </p>
      )}
    </WarningNote>
  );
}

/**
 * The redirect URI as a row of the provider sheet, with its copy button (#2083).
 *
 * An identity provider asks for it before it issues the client ID and secret
 * this form wants, so the add sheet shows it from the slug as it is typed
 * rather than only on the card of a provider that already exists. It is not an
 * input: nothing here is editable, and a disabled field would read as refused
 * rather than derived. The value is `select-all`, so on a plain-http dashboard,
 * where the clipboard API is withheld, it can still be copied by hand.
 */
function RedirectUriRow({
  value,
  invalid = false,
  hint,
  children,
}: {
  /** null until there is a valid slug to build it from */
  value: string | null;
  /** the slug typed is one the server refuses, so there is nothing to offer for
   * copying: a redirect uri registered for it would never work (#2304) */
  invalid?: boolean;
  hint: string;
  /** a note under the hint: why the value is incomplete or only a default */
  children?: React.ReactNode;
}) {
  const { t } = useTranslation();
  const labelId = React.useId();
  const hintId = React.useId();
  return (
    <div role="group" aria-labelledby={labelId} aria-describedby={hintId} className="space-y-1.5">
      <p id={labelId} className="text-sm font-medium leading-none">
        {t("pages.sso.create.redirectUri")}
      </p>
      <div className="flex min-h-9 min-w-0 items-center gap-1 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-base)] py-1 pl-3 pr-1">
        {value ? (
          <>
            <span className="min-w-0 flex-1 select-all break-all font-mono text-xs text-foreground">
              {value}
            </span>
            <CopyButton value={value} label={t("pages.sso.providers.copyRedirectUri")} />
          </>
        ) : (
          <span className="text-sm text-muted-foreground">
            {invalid
              ? t("pages.sso.create.redirectUriInvalid")
              : t("pages.sso.create.redirectUriEmpty")}
          </span>
        )}
      </div>
      <p id={hintId} className="text-xs text-muted-foreground">
        {hint}
      </p>
      {children}
    </div>
  );
}

/**
 * The two `mfa_policy` values that make a factor mandatory: a member without
 * one is walked through enrolment at their next sign-in before they get a
 * session (#1852).
 *
 * `optional` does not: it changes who *may* enrol, not who has to.
 */
const MFA_LOCKS_OUT: MfaPolicy[] = ["required_superadmin", "required_all"];

/**
 * Catalog key per policy value.
 *
 * Not the wire value itself: i18next reads a trailing `_all` or `_one` as a
 * plural suffix, so `mfaOptions.required_all` would be a key the catalogs and
 * the parity gate disagree about.
 */
const MFA_KEY: Record<MfaPolicy, string> = {
  off: "off",
  optional: "optional",
  required_superadmin: "requiredSuperadmin",
  required_all: "requiredAll",
};

/**
 * The break-glass procedure, for the confirmation that warns about a lockout.
 *
 * A link to our own docs on the forge rather than to a docs site this
 * deployment may not be able to reach — and the command itself is in the copy,
 * so an operator with no network still knows what to run.
 */
const MFA_DOCS_URL =
  "https://github.com/rolter-ai/rolter/blob/master/docs/user-docs/security/two-factor-authentication.mdx#break-glass-a-lost-device";

/**
 * How long an org may give its members before a `required_*` policy starts
 * sending the unenrolled through enrolment (#1852), in days.
 *
 * Presets rather than a date picker: the decision is "how much notice", and a
 * calendar invites a precision nobody needs while making "next Tuesday at
 * midnight in whose timezone" the admin's problem.
 */
const GRACE_DAYS = [7, 14, 30];

/**
 * The confirmations a policy save can raise, named for the change each one
 * guards. They are asked in this order.
 */
type Confirmation = "password" | "sso" | "mfa";

/**
 * The org's announced start, when it is still ahead. A window that has passed
 * reads as no window at all — the requirement already applies — so it is not
 * offered as something to keep.
 */
function pendingWindow(policy: OrgAuthPolicy): string | null {
  const at = policy.mfa_enforce_after;
  return at && Date.parse(at) > Date.now() ? at : null;
}

/**
 * What `mfa_enforce_after` to send for a grace choice: `keep` the pending
 * window, start `now` (null), or a number of days from the moment of saving.
 */
function graceDeadline(grace: string, pending: string | null): string | null {
  if (grace === "keep") return pending;
  if (grace === "now") return null;
  return new Date(Date.now() + Number(grace) * 86_400_000).toISOString();
}

/**
 * Which ways into the dashboard this org allows, and what it demands on the
 * way in.
 *
 * The two flags are sent together because the control plane refuses the
 * *combination*, not the field: both off is an outage, and passwords off before
 * an enabled provider exists locks every non-superadmin out. Each is a 409 with
 * its own message, so the local guard below only covers the case an operator
 * can see for themselves.
 *
 * `mfa_policy` travels with them (#1078), and with it the grace window
 * (#1852). A `required_*` value sends anyone without an armed factor through
 * enrolment before they get a session, so tightening it confirms first, and
 * the confirmation says when it starts and names the way back in for a lost
 * device.
 *
 * Turning password sign-in off confirms as well (#2084): from then on every
 * member but a superadmin signs in through an identity provider. The control
 * plane refuses it with no enabled provider, but accepts it when the only one
 * has no client secret, so the confirmation names any enabled provider that
 * would fail its token exchange.
 *
 * So does turning single sign-on off (#2326), when the org has an enabled
 * provider: the callback refuses every provider while it is off, and an account
 * a provider created has no password, so those members cannot sign in at all.
 * A save that needs several confirmations asks them one after another and sends
 * one request after the last.
 */
function SignInPolicyCard({
  orgId,
  policy,
  providers,
}: {
  orgId: string;
  policy: OrgAuthPolicy;
  providers: SsoProviderRow[];
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const [password, setPassword] = React.useState(policy.allow_password_login);
  const [sso, setSso] = React.useState(policy.allow_sso);
  const [mfa, setMfa] = React.useState<MfaPolicy>(policy.mfa_policy);
  const pending = pendingWindow(policy);
  const initialGrace = pending ? "keep" : "now";
  const [grace, setGrace] = React.useState(initialGrace);
  // which confirmation is up
  const [confirming, setConfirming] = React.useState<Confirmation | null>(null);
  const fmt = useFormat();

  // re-seed when the server's copy moves under us — another admin, or our own
  // save coming back
  React.useEffect(() => {
    setPassword(policy.allow_password_login);
    setSso(policy.allow_sso);
    setMfa(policy.mfa_policy);
    setGrace(initialGrace);
  }, [policy.allow_password_login, policy.allow_sso, policy.mfa_policy, initialGrace]);

  const requires = MFA_LOCKS_OUT.includes(mfa);
  // the grace choice only means something while a factor is required
  const graceDirty = requires && grace !== initialGrace;
  // when the requirement starts, as the confirmation should say it
  const deadline = requires ? graceDeadline(grace, pending) : null;
  // an announced window moved earlier binds members sooner than they were
  // told, whether to "at their next sign-in" or to a nearer preset
  const pullsIn =
    pending !== null && (deadline === null || Date.parse(deadline) < Date.parse(pending));
  // only a *tightening* is worth a confirmation: turning a requirement on, or
  // cutting an announced window short. Relaxing the policy or moving a date
  // later binds nobody sooner, and a dialog in front of it would be the
  // click-through that teaches people to dismiss the one that matters
  const tightens = requires && (mfa !== policy.mfa_policy || pullsIn);
  // only the switch going from on to off: a policy saved with passwords already
  // off, for some other field, takes nobody's route away
  const turnsPasswordOff = policy.allow_password_login && !password;
  // likewise only the flip from on to off, and only with a provider to shut out
  const turnsSsoOff = locksOutSsoMembers(providers, policy, { allow_sso: sso });
  const gap = secretGap(providers);

  // how many accounts the tightening would bind. Best-effort: a caller who may
  // not read the org's memberships still gets the warning, just without a
  // number in it — refusing to warn at all would be the worse trade
  const members = useQuery({
    queryKey: ["memberships", orgId],
    queryFn: () => fetchMemberships(orgId),
    enabled: tightens,
    retry: false,
  });

  const save = useMutation({
    mutationFn: () =>
      updateAuthPolicy(orgId, {
        allow_password_login: password,
        allow_sso: sso,
        mfa_policy: mfa,
        // computed at the moment of saving, so "in 7 days" counts from the
        // click rather than from when the card rendered
        mfa_enforce_after: requires ? graceDeadline(grace, pending) : null,
      }),
    onSuccess: (next) => {
      setConfirming(null);
      queryClient.setQueryData([POLICY_KEY, orgId], next);
      void queryClient.invalidateQueries({ queryKey: [POLICY_KEY, orgId] });
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.signInPolicy") }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.signInPolicy") }),
        detail: errorDetail(error),
      });
    },
  });

  // the confirmations this save raises, in the order they are asked. the
  // password and single sign-on ones never meet, since both off is refused
  // before the save, but nothing here depends on it
  const steps: Confirmation[] = [];
  if (turnsPasswordOff) steps.push("password");
  if (turnsSsoOff) steps.push("sso");
  if (tightens) steps.push("mfa");
  // an answer moves on to the next confirmation, and the last one sends the
  // request. a confirmation that only moves on runs none, which it says to
  // `ConfirmDialog` by passing no `pending`
  const advance = (from: Confirmation) => {
    const next = steps[steps.indexOf(from) + 1];
    if (next) setConfirming(next);
    else save.mutate();
  };
  const sendsRequest = (step: Confirmation) => steps.indexOf(step) === steps.length - 1;

  const dirty =
    password !== policy.allow_password_login ||
    sso !== policy.allow_sso ||
    mfa !== policy.mfa_policy ||
    graceDirty;
  const bothOff = !password && !sso;

  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]">
      <header className="border-b border-[color:var(--border-subtle)] px-4 py-3">
        <h2 className="text-sm font-medium text-foreground">{t("pages.sso.policy.title")}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{t("pages.sso.policy.subtitle")}</p>
      </header>
      <div className="flex flex-col gap-4 px-4 py-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-foreground">
              {t("pages.sso.policy.passwordLabel")}
            </p>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("pages.sso.policy.passwordHint")}
            </p>
          </div>
          <Switch
            checked={password}
            onCheckedChange={setPassword}
            aria-label={t("pages.sso.policy.passwordLabel")}
          />
        </div>
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-foreground">{t("pages.sso.policy.ssoLabel")}</p>
            <p className="mt-1 text-sm text-muted-foreground">{t("pages.sso.policy.ssoHint")}</p>
          </div>
          <Switch
            checked={sso}
            onCheckedChange={setSso}
            aria-label={t("pages.sso.policy.ssoLabel")}
          />
        </div>
        {/* not a switch: four values, and the two `required_*` ones differ in
            who they bind rather than in how much they do */}
        <Field
          label={t("pages.sso.policy.mfaLabel")}
          hint={t(`pages.sso.policy.mfaHints.${MFA_KEY[mfa]}`)}
        >
          <Combobox
            value={mfa}
            onChange={(picked) => setMfa(picked as MfaPolicy)}
            options={MFA_POLICIES.map((value) => ({
              value,
              label: t(`pages.sso.policy.mfaOptions.${MFA_KEY[value]}`),
            }))}
          />
        </Field>
        {/* how much notice the members get (#1852). only while a factor is
            required: under `off` and `optional` there is nothing to postpone,
            and the control plane drops a window sent with them */}
        {requires && (
          <Field label={t("pages.sso.policy.graceLabel")} hint={t("pages.sso.policy.graceHint")}>
            <Combobox
              value={grace}
              onChange={setGrace}
              options={[
                ...(pending
                  ? [
                      {
                        value: "keep",
                        label: t("pages.sso.policy.graceOptions.keep", {
                          date: fmt.date(pending),
                        }),
                      },
                    ]
                  : []),
                { value: "now", label: t("pages.sso.policy.graceOptions.now") },
                ...GRACE_DAYS.map((days) => ({
                  value: String(days),
                  label: t("pages.sso.policy.graceOptions.days", { count: days }),
                })),
              ]}
            />
          </Field>
        )}
      </div>
      <footer className="flex flex-wrap items-center gap-3 border-t border-[color:var(--border-subtle)] px-4 py-3">
        {bothOff && (
          <p className="text-xs text-[color:var(--status-warning-text)]">
            {t("pages.sso.policy.bothOff")}
          </p>
        )}
        {/* the control plane's own words, never a gloss on them */}
        {save.isError && (
          <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
            {(save.error as Error).message}
          </p>
        )}
        <GatedButton
          gate="org_auth_policy:update"
          control="sso-policy-save"
          className="ml-auto"
          size="sm"
          disabled={!dirty || bothOff || save.isPending}
          onClick={() => {
            save.reset();
            if (steps.length > 0) setConfirming(steps[0]);
            else save.mutate();
          }}
        >
          {save.isPending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("pages.sso.policy.save")}
        </GatedButton>
      </footer>

      <ConfirmDialog
        name="sso-password-off"
        open={confirming === "password"}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={t("pages.sso.policy.passwordConfirm.title")}
        description={t("pages.sso.policy.passwordConfirm.body")}
        confirmLabel={t("pages.sso.policy.passwordConfirm.confirm")}
        pending={sendsRequest("password") ? save.isPending : undefined}
        error={save.error}
        onConfirm={() => advance("password")}
      >
        {gap.missing.length > 0 && <NoSecretNotice gap={gap} />}
      </ConfirmDialog>

      <ConfirmDialog
        name="sso-single-sign-on-off"
        open={confirming === "sso"}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={t("pages.sso.policy.ssoConfirm.title")}
        description={t("pages.sso.policy.ssoConfirm.body")}
        confirmLabel={t("pages.sso.policy.ssoConfirm.confirm")}
        pending={sendsRequest("sso") ? save.isPending : undefined}
        error={save.error}
        onConfirm={() => advance("sso")}
      >
        <SsoOffNotice />
      </ConfirmDialog>

      <ConfirmDialog
        name="sso-mfa-policy"
        open={confirming === "mfa"}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={t("pages.sso.policy.mfaConfirm.title")}
        description={
          members.data
            ? t("pages.sso.policy.mfaConfirm.bodyWithCount", {
                // a person with a role on the org and another on a team is two
                // rows, and one member
                count: distinctPeople(members.data),
              })
            : t("pages.sso.policy.mfaConfirm.body")
        }
        confirmLabel={t("pages.sso.policy.mfaConfirm.confirm")}
        pending={save.isPending}
        error={save.error}
        onConfirm={() => save.mutate()}
      >
        {/* the date, when there is one: the body says what happens, this says
            when, and an admin reading "at their next sign-in" while having
            picked a window would think the window had been ignored */}
        {deadline && (
          <p className="text-sm text-muted-foreground">
            {t("pages.sso.policy.mfaConfirm.grace", { date: fmt.date(deadline) })}
          </p>
        )}
        {/* the way back in for a member who loses their device, named before
            it happens rather than after */}
        <p className="text-xs text-muted-foreground">
          {t("pages.sso.policy.mfaConfirm.breakGlass")}{" "}
          <a
            href={MFA_DOCS_URL}
            target="_blank"
            rel="noreferrer"
            className="underline underline-offset-4 hover:text-foreground"
          >
            {t("pages.sso.policy.mfaConfirm.breakGlassLink")}
          </a>
        </p>
      </ConfirmDialog>
    </section>
  );
}

/**
 * The IdP groups this provider turns into roles.
 *
 * A mapping grants at the provider's own org by default — the create endpoint
 * reads an omitted scope that way — but it may also name one team or one
 * project inside that org (#1234). The form and the list are the shared
 * `GroupMappings`; this is the strip of the provider's card they sit in, and
 * the words that are true of this screen only: a mapping grants at each
 * member's next sign-in, and the empty list falls back to the provider's
 * default role.
 */
function ProviderMappings({ provider }: { provider: SsoProviderRow }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2.5 border-t border-[color:var(--border-subtle)] px-4 py-3.5">
      <div className="flex items-center gap-1.5">
        <Users aria-hidden className="h-3.5 w-3.5 text-muted-foreground" />
        <h3 className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
          {t("pages.sso.mappings.title")}
        </h3>
      </div>
      <GroupMappings
        kind="sso"
        orgId={provider.org_id}
        queryKey={[MAPPINGS_KEY, provider.id]}
        fetchMappings={() => fetchSsoGroupMappings(provider.id)}
        createMapping={(grant) => createSsoGroupMapping(provider.id, grant)}
        deleteMapping={deleteSsoGroupMapping}
        empty={
          provider.default_role
            ? t("pages.sso.mappings.emptyWithDefault", {
                role: roleLabel(t, provider.default_role),
              })
            : t("pages.sso.mappings.empty")
        }
        grantTiming={t("pages.sso.mappings.grantTiming")}
        removeBody={(role, scope) => t("pages.sso.mappings.removeBody", { role, scope })}
        // every provider card carries one of these, so the label names which
        // one: "Map group" alone is ambiguous the moment an org registers a
        // second identity provider
        addLabel={t("pages.sso.mappings.addNamed", { provider: provider.name })}
      />
    </div>
  );
}

function ProviderCard({
  provider,
  onClearSecret,
  onDelete,
  onEdit,
  onToggle,
  clearingSecret,
  deleting,
  toggling,
}: {
  provider: SsoProviderRow;
  onClearSecret: (provider: SsoProviderRow) => void;
  onDelete: (provider: SsoProviderRow) => void;
  onEdit: (provider: SsoProviderRow) => void;
  onToggle: (provider: SsoProviderRow, enabled: boolean) => void;
  clearingSecret: boolean;
  deleting: boolean;
  toggling: boolean;
}) {
  const { t } = useTranslation();

  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]">
      <header className="flex items-start gap-3 px-4 py-3.5">
        <KeyRound aria-hidden className="mt-0.5 h-4 w-4 flex-none text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-foreground">{provider.name}</span>
            <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
              {provider.slug}
            </Pill>
            {provider.enabled ? (
              <Badge dot tone="success">
                {t("pages.sso.providers.enabled")}
              </Badge>
            ) : (
              <Badge tone="neutral">{t("pages.sso.providers.disabled")}</Badge>
            )}
            {/* a provider with no sealed secret cannot complete the token
                exchange; without this badge the first symptom is a failed
                login, long after whoever registered it has moved on (#1231) */}
            {!provider.has_client_secret && (
              <Badge tone="warning" title={t("pages.sso.providers.noSecretHint")}>
                {t("pages.sso.providers.noSecret")}
              </Badge>
            )}
          </div>
          <p className="mt-1 text-sm text-muted-foreground">
            {provider.default_role
              ? t("pages.sso.providers.defaultRole", {
                  role: roleLabel(t, provider.default_role),
                })
              : t("pages.sso.providers.noDefaultRole")}
          </p>
        </div>
        {/* taking a provider out of service is a routine act — an IdP
            migration, a broken secret — and used to require deleting it,
            which took its group mappings with it (#1233). it is an update, the
            same capability as the edit beside it (#2084) */}
        <GatedSwitch
          gate="sso_provider:update"
          control="sso-provider-toggle"
          checked={provider.enabled}
          disabled={toggling}
          onCheckedChange={(next) => onToggle(provider, next)}
          aria-label={t("pages.sso.providers.toggleNamed", { name: provider.name })}
        />
        <RowIconButton
          gate="sso_provider:update"
          control="sso-provider-edit"
          title={t("pages.sso.providers.edit")}
          aria-label={t("pages.sso.providers.editNamed", { name: provider.name })}
          onClick={() => onEdit(provider)}
        >
          <Pencil className="h-3.5 w-3.5" />
        </RowIconButton>
        {/* the only deliberate way to drop a sealed secret. it exists because
            the edit form no longer can: a whitespace-only field there used to
            trim to "" and clear the secret silently, and the first symptom was
            a failed login (#1293). offered only where there is one to remove */}
        {provider.has_client_secret && (
          <RowIconButton
            gate="sso_provider:update"
            control="sso-provider-secret-clear"
            title={t("pages.sso.providers.clearSecret")}
            aria-label={t("pages.sso.providers.clearSecretNamed", {
              name: provider.name,
            })}
            disabled={clearingSecret}
            onClick={() => onClearSecret(provider)}
          >
            {clearingSecret ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Eraser className="h-3.5 w-3.5" />
            )}
          </RowIconButton>
        )}
        <RowIconButton
          danger
          gate="sso_provider:delete"
          control="sso-provider-delete"
          title={t("pages.sso.providers.delete")}
          aria-label={t("pages.sso.providers.deleteNamed", { name: provider.name })}
          disabled={deleting}
          onClick={() => onDelete(provider)}
        >
          {deleting ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Trash2 className="h-3.5 w-3.5" />
          )}
        </RowIconButton>
      </header>

      <div className="flex flex-col gap-1.5 border-t border-[color:var(--border-subtle)] px-4 py-3">
        <Detail label={t("pages.sso.providers.issuer")} value={provider.issuer} />
        <Detail label={t("pages.sso.providers.clientId")} value={provider.client_id} />
        <Detail
          label={t("pages.sso.providers.clientSecret")}
          value={
            provider.has_client_secret
              ? t("pages.sso.providers.secretStored")
              : t("pages.sso.providers.secretMissing")
          }
        />
        {/* both addresses come from the control plane, built from its
            configured public url by the functions the login flow itself uses.
            they used to be assembled from the browser's origin, which is wrong
            behind a proxy, and the only one shown was the login url, which is
            not what an identity provider asks for (#2083) */}
        <Detail
          label={t("pages.sso.providers.redirectUri")}
          value={provider.redirect_uri}
          copyLabel={t("pages.sso.providers.copyRedirectUri")}
          note={t("pages.sso.providers.redirectUriNote")}
          wrap
        />
        <Detail
          label={t("pages.sso.providers.startUrl")}
          value={provider.login_url}
          copyLabel={t("pages.sso.providers.copyStartUrl")}
          note={t("pages.sso.providers.startUrlNote")}
          wrap
        />
        <Detail label={t("pages.sso.providers.groupClaim")} value={provider.group_claim} />
        <Detail label={t("pages.sso.providers.scopes")} value={provider.scopes.join(" ")} />
      </div>

      <ProviderMappings provider={provider} />
    </section>
  );
}

interface Draft {
  name: string;
  slug: string;
  issuer: string;
  clientId: string;
  clientSecret: string;
  scopes: string;
  groupClaim: string;
  defaultRole: string;
}

const EMPTY_DRAFT: Draft = {
  name: "",
  slug: "",
  issuer: "",
  clientId: "",
  clientSecret: "",
  scopes: "",
  groupClaim: "",
  defaultRole: "",
};

const draftFrom = (provider: SsoProviderRow): Draft => ({
  name: provider.name,
  slug: provider.slug,
  issuer: provider.issuer,
  clientId: provider.client_id,
  // never prefilled: the sealed secret is not readable, so an empty field
  // here means "leave the stored one alone" rather than "clear it"
  clientSecret: "",
  scopes: provider.scopes.join(" "),
  groupClaim: provider.group_claim,
  defaultRole: provider.default_role ?? "",
});

// register or edit a provider. the client secret is sealed with the KEK on the
// way in and never serialized on the way out, so this form is the only place it
// is ever legible.
//
// editing exists because the alternative was delete-and-recreate, which drops
// every group mapping hanging off the provider and changes its id in the audit
// trail — a heavy price for a rotated secret or a mistyped issuer (#1233)
function ProviderSheet({
  open,
  onOpenChange,
  orgId,
  provider,
  publicUrl,
  publicUrlFailed,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  /** the provider being edited, or null to register a new one */
  provider: SsoProviderRow | null;
  /** the control plane's public base, for the redirect uri preview; absent
   * while it loads or when it could not be read */
  publicUrl: PublicUrl | undefined;
  publicUrlFailed: boolean;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const toast = useToast();
  const slugId = React.useId();
  const slugHintId = React.useId();
  const slugErrorId = React.useId();
  const editing = !!provider;
  const initial = React.useMemo(() => (provider ? draftFrom(provider) : EMPTY_DRAFT), [provider]);
  const [draft, setDraft] = React.useState<Draft>(initial);

  React.useEffect(() => {
    if (open) setDraft(initial);
  }, [open, initial]);

  const scopeList = () =>
    draft.scopes.trim()
      ? draft.scopes
          .trim()
          .split(/[\s,]+/)
          .filter(Boolean)
      : undefined;

  const save = useMutation({
    mutationFn: () =>
      provider
        ? updateSsoProvider(provider.id, {
            name: draft.name.trim(),
            issuer: draft.issuer.trim(),
            client_id: draft.clientId.trim(),
            // an untouched field leaves the sealed secret where it is; the
            // form cannot show it, so it must not be able to erase it either.
            // `trim()` before the check, not after: a stray space or a pasted
            // newline used to survive the truthiness test and reach the server
            // as "", which is the wire's "clear it" (#1293). clearing is the
            // card's own control now, never a side effect of saving the form
            client_secret: draft.clientSecret.trim() || undefined,
            scopes: scopeList(),
            group_claim: draft.groupClaim.trim() || undefined,
            default_role: draft.defaultRole || undefined,
            enabled: provider.enabled,
          })
        : createSsoProvider(orgId, {
            name: draft.name.trim(),
            slug: draft.slug.trim(),
            issuer: draft.issuer.trim(),
            client_id: draft.clientId.trim(),
            // an omitted secret is a public client; an empty string is not sent
            // so the server does not seal a blank
            client_secret: draft.clientSecret.trim() || undefined,
            scopes: scopeList(),
            group_claim: draft.groupClaim.trim() || undefined,
            default_role: draft.defaultRole || undefined,
          }),
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: editing ? t("toast.saved") : t("toast.created", { what: draft.name.trim() }),
        detail: editing ? t("toast.savedDetail", { what: draft.name.trim() }) : undefined,
      });
      onSaved();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: draft.name.trim() }),
        detail: errorDetail(error),
      });
    },
  });

  const set = (patch: Partial<Draft>) => setDraft((d) => ({ ...d, ...patch }));
  const dirty = editing
    ? (Object.keys(draft) as (keyof Draft)[]).some((k) => draft[k] !== initial[k])
    : Object.values(draft).some((v) => v !== "");

  // the slug is checked as it will be sent, which is trimmed like every other
  // field, and never rewritten: it is registered at the identity provider, so
  // the admin has to see exactly what will be saved. a saved provider's slug
  // cannot change, so only a new one is checked (#2304)
  const slug = draft.slug.trim();
  const slugProblem = editing ? null : ssoSlugProblem(slug);
  const slugInvalid = slugProblem === "charset" || slugProblem === "length";
  const suggestion = slugProblem === "charset" ? suggestSsoSlug(slug) : null;
  const slugError =
    slugProblem === "length"
      ? t("pages.sso.create.slugTooLong", {
          max: fmt.number(SSO_SLUG_MAX),
          length: fmt.number(slug.length),
        })
      : slugProblem === "charset"
        ? suggestion
          ? t("pages.sso.create.slugSuggest", { suggestion })
          : t("pages.sso.create.slugInvalid")
        : undefined;

  const canSave =
    !!draft.name.trim() &&
    (editing || slugProblem === null) &&
    !!draft.issuer.trim() &&
    !!draft.clientId.trim();

  // a saved provider carries the server's own redirect uri. a new one is
  // previewed from the typed slug on the server's public base, never the
  // browser's origin; with the base unread, only the path is honest to show.
  // a slug the server refuses previews nothing, so no uri is copied for it
  const redirect = provider
    ? provider.redirect_uri
    : slug && !slugInvalid
      ? ssoRedirectUri(publicUrl?.public_url ?? "", slug)
      : null;
  const redirectNote =
    !provider && publicUrlFailed
      ? "pages.sso.create.redirectUriUnknown"
      : publicUrl?.configured === false
        ? "pages.sso.create.redirectUriDefault"
        : null;

  return (
    <EditorSheet
      name={editing ? "sso-connection-edit" : "sso-connection-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("pages.sso.edit.title") : t("pages.sso.create.title")}
      subtitle={editing ? t("pages.sso.edit.subtitle") : t("pages.sso.create.subtitle")}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={editing ? t("pages.sso.edit.save") : t("pages.sso.create.save")}
      canSave={canSave}
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <Field label={t("pages.sso.create.name")} hint={t("pages.sso.create.nameHint")}>
        <Input
          value={draft.name}
          onChange={(e) => set({ name: e.target.value })}
          placeholder={t("pages.sso.create.namePlaceholder")}
        />
      </Field>
      <Field label={t("pages.sso.create.slug")} htmlFor={slugId}>
        <Input
          id={slugId}
          value={draft.slug}
          disabled={editing}
          aria-invalid={slugInvalid || undefined}
          aria-describedby={describedBy(slugHintId, slugInvalid && slugErrorId)}
          onChange={(e) => set({ slug: e.target.value })}
          placeholder={t("pages.sso.create.slugPlaceholder")}
        />
        <p id={slugHintId} className="text-xs text-muted-foreground">
          {editing ? t("pages.sso.edit.slugImmutable") : t("pages.sso.create.slugHint")}
        </p>
        <FieldError id={slugErrorId} error={slugError} />
      </Field>
      <RedirectUriRow
        value={redirect}
        invalid={slugInvalid}
        hint={editing ? t("pages.sso.edit.redirectUriHint") : t("pages.sso.create.redirectUriHint")}
      >
        {redirectNote && (
          <p className="text-xs text-[color:var(--status-warning-text)]">
            <Trans
              i18nKey={redirectNote}
              values={{ url: publicUrl?.public_url ?? "" }}
              components={{ code: <code className="font-mono" /> }}
            />
          </p>
        )}
      </RedirectUriRow>
      <Field label={t("pages.sso.create.issuer")} hint={t("pages.sso.create.issuerHint")}>
        <Input
          value={draft.issuer}
          onChange={(e) => set({ issuer: e.target.value })}
          placeholder={t("pages.sso.create.issuerPlaceholder")}
        />
      </Field>
      <Field label={t("pages.sso.create.clientId")} hint={t("pages.sso.create.clientIdHint")}>
        <Input
          value={draft.clientId}
          onChange={(e) => set({ clientId: e.target.value })}
          placeholder={t("pages.sso.create.clientIdPlaceholder")}
        />
      </Field>
      <Field
        label={t("pages.sso.create.clientSecret")}
        hint={
          editing ? t("pages.sso.edit.clientSecretHint") : t("pages.sso.create.clientSecretHint")
        }
      >
        <Input
          type="password"
          value={draft.clientSecret}
          onChange={(e) => set({ clientSecret: e.target.value })}
        />
      </Field>
      <p className="text-sm font-medium text-[color:var(--status-warning-text)]">
        {t("pages.sso.create.secretWriteOnly")}
      </p>
      <Field label={t("pages.sso.create.scopes")} hint={t("pages.sso.create.scopesHint")}>
        <Input
          value={draft.scopes}
          onChange={(e) => set({ scopes: e.target.value })}
          placeholder={t("pages.sso.create.scopesPlaceholder")}
        />
      </Field>
      <Field label={t("pages.sso.create.groupClaim")} hint={t("pages.sso.create.groupClaimHint")}>
        <Input
          value={draft.groupClaim}
          onChange={(e) => set({ groupClaim: e.target.value })}
          placeholder={t("pages.sso.create.groupClaimPlaceholder")}
        />
      </Field>
      <Field label={t("pages.sso.create.defaultRole")} hint={t("pages.sso.create.defaultRoleHint")}>
        <Combobox
          value={draft.defaultRole}
          onChange={(defaultRole) => set({ defaultRole })}
          options={[
            { value: "", label: t("pages.sso.create.defaultRoleNone") },
            ...MAPPABLE_ROLES.map((r) => ({ value: r, label: roleLabel(t, r) })),
          ]}
        />
      </Field>
    </EditorSheet>
  );
}

/**
 * Governance › Single sign-on (#1185).
 *
 * The control plane has carried OIDC SSO and a per-org sign-in policy since
 * #240, but nothing in the dashboard could register a provider: the login
 * screen rendered "Continue with …" buttons for providers only a direct API
 * call could create. This is that screen.
 */
export default function SingleSignOn() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  const orgId = scope.orgId;

  const providers = useQuery({
    queryKey: [PROVIDERS_KEY, orgId],
    queryFn: () => fetchSsoProviders(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const policy = useQuery({
    queryKey: [POLICY_KEY, orgId],
    queryFn: () => fetchAuthPolicy(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  // the base every URL here is built from (#2083). the cards read theirs off
  // the provider rows, so this only feeds the add sheet's preview and the
  // notice for an unset ROLTER_PUBLIC_URL, and the screen does not wait on it
  const publicUrl = usePublicUrl();

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // the provider list is what the user is actually waiting on here
  useScreenReady(!providers.isLoading);
  useErrorState(!!providers.error, "sso");

  const invalidate = () => queryClient.invalidateQueries({ queryKey: [PROVIDERS_KEY, orgId] });

  const remove = useMutation({
    mutationFn: (id: string) => deleteSsoProvider(id),
    onSuccess: invalidate,
  });

  // the switch on a card sends the row back unchanged except for `enabled`,
  // and omits `client_secret` so the sealed one is left alone (#1233)
  const toggle = useMutation({
    mutationFn: ({ provider, enabled }: { provider: SsoProviderRow; enabled: boolean }) =>
      updateSsoProvider(provider.id, {
        name: provider.name,
        issuer: provider.issuer,
        client_id: provider.client_id,
        scopes: provider.scopes,
        group_claim: provider.group_claim,
        default_role: provider.default_role ?? undefined,
        enabled,
      }),
    onSuccess: (updated) => {
      invalidate();
      toast.push({
        tone: "success",
        title: updated.enabled
          ? t("pages.sso.providers.enabledToast", { name: updated.name })
          : t("pages.sso.providers.disabledToast", { name: updated.name }),
      });
    },
    onError: (error, { provider }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: provider.name }),
        detail: errorDetail(error),
      });
    },
  });

  // #1293: the deliberate way to turn a provider into a public client. the
  // empty string is the third value `PUT /sso-providers/:id` understands —
  // omitted keeps the sealed secret, a value rotates it, "" drops it — and
  // nothing else in the screen is allowed to send it
  const clearSecret = useMutation({
    mutationFn: (provider: SsoProviderRow) =>
      updateSsoProvider(provider.id, {
        name: provider.name,
        issuer: provider.issuer,
        client_id: provider.client_id,
        client_secret: "",
        scopes: provider.scopes,
        group_claim: provider.group_claim,
        default_role: provider.default_role ?? undefined,
        enabled: provider.enabled,
      }),
    // the refetch is what flips the `has_client_secret` badge on the card
    onSuccess: invalidate,
  });

  const [sheetOpen, setSheetOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<SsoProviderRow | null>(null);
  const openCreate = () => {
    setEditing(null);
    setSheetOpen(true);
  };
  const openEdit = (provider: SsoProviderRow) => {
    setEditing(provider);
    setSheetOpen(true);
  };
  const [deleteTarget, setDeleteTarget] = React.useState<SsoProviderRow | null>(null);
  const startDelete = (provider: SsoProviderRow) => {
    remove.reset();
    setDeleteTarget(provider);
  };
  // the provider a switch was flipped off on, waiting for the confirmation.
  // turning one back on restores a route and sends at once (#2084)
  const [disableTarget, setDisableTarget] = React.useState<SsoProviderRow | null>(null);
  const switchProvider = (provider: SsoProviderRow, enabled: boolean) => {
    if (enabled) {
      toggle.mutate({ provider, enabled });
      return;
    }
    toggle.reset();
    setDisableTarget(provider);
  };
  const [secretTarget, setSecretTarget] = React.useState<SsoProviderRow | null>(null);
  const startClearSecret = (provider: SsoProviderRow) => {
    clearSecret.reset();
    setSecretTarget(provider);
  };

  if (scope.isLoading || (!!orgId && (providers.isLoading || policy.isLoading))) {
    return (
      <PageBody>
        <Skeleton className="h-[196px] rounded-[10px]" />
        <Skeleton className="h-9 w-[280px] rounded-md" />
        <Skeleton className="h-[240px] rounded-[10px]" />
      </PageBody>
    );
  }

  const rows = providers.data ?? [];
  // no org means nothing to hang a provider on, and an unreadable list means
  // this principal may not manage them either
  const canManage = !!orgId && !providers.isError;
  // against the saved policy: with it unread there is nothing to warn from, and
  // the plain confirmation still stands
  const locksOut = (target: SsoProviderRow | null) =>
    !!target && !!policy.data && locksOutMembers(rows, target, policy.data);
  const disableLocksOut = locksOut(disableTarget);
  const deleteLocksOut = locksOut(deleteTarget);

  return (
    <PageBody>
      {policy.isError && (
        <LoadError
          error={policy.error}
          resource={t("errors.resources.signInPolicy")}
          onRetry={() => policy.refetch()}
        />
      )}
      {policy.data && orgId && (
        <SignInPolicyCard orgId={orgId} policy={policy.data} providers={rows} />
      )}

      <div className="flex flex-wrap items-center gap-3">
        <h2 className="text-sm font-medium text-foreground">{t("pages.sso.providers.title")}</h2>
        <ListSummary data={providers.data}>
          {(all) => t("pages.sso.providers.count", { count: all.length })}
        </ListSummary>
        <GatedButton
          gate="sso_provider:create"
          control="sso-provider-new"
          className="ml-auto"
          disabled={!canManage}
          onClick={openCreate}
        >
          <Plus className="h-4 w-4" aria-hidden />
          {t("pages.sso.providers.add")}
        </GatedButton>
      </div>

      {/* only beside a list this caller may read: a member refused the
          providers has no URL here to be warned about */}
      {!providers.isError && publicUrl.data?.configured === false && (
        <PublicUrlNotice publicUrl={publicUrl.data} />
      )}

      {providers.isError && (
        <LoadError
          error={providers.error}
          resource={t("errors.resources.ssoProviders")}
          onRetry={() => providers.refetch()}
        />
      )}

      {!providers.isError &&
        (rows.length === 0 ? (
          <EmptyState
            uxTarget="sso-providers"
            icon={<ShieldCheck />}
            title={t("pages.sso.empty.title")}
            description={t("pages.sso.empty.body")}
            actions={
              <GatedButton
                gate="sso_provider:create"
                control="sso-provider-new-empty"
                disabled={!canManage}
                onClick={openCreate}
              >
                <Plus className="h-4 w-4" aria-hidden />
                {t("pages.sso.providers.add")}
              </GatedButton>
            }
          />
        ) : (
          <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(420px,100%),1fr))]">
            {rows.map((provider) => (
              <ProviderCard
                key={provider.id}
                provider={provider}
                clearingSecret={clearSecret.isPending && clearSecret.variables?.id === provider.id}
                deleting={remove.isPending && remove.variables === provider.id}
                toggling={toggle.isPending && toggle.variables?.provider.id === provider.id}
                onClearSecret={startClearSecret}
                onDelete={startDelete}
                onEdit={openEdit}
                onToggle={switchProvider}
              />
            ))}
          </div>
        ))}

      {orgId && (
        <ProviderSheet
          open={sheetOpen}
          onOpenChange={setSheetOpen}
          orgId={orgId}
          provider={editing}
          publicUrl={publicUrl.data}
          publicUrlFailed={publicUrl.isError}
          onSaved={invalidate}
        />
      )}

      <ConfirmDialog
        name="sso-secret-clear"
        open={!!secretTarget}
        onOpenChange={(open) => !open && setSecretTarget(null)}
        title={t("pages.sso.clearSecret.title", { name: secretTarget?.name })}
        description={t("pages.sso.clearSecret.body")}
        confirmLabel={t("pages.sso.clearSecret.confirm")}
        pending={clearSecret.isPending}
        error={clearSecret.error}
        onConfirm={() => {
          if (!secretTarget) return;
          const name = secretTarget.name;
          clearSecret.mutate(secretTarget, {
            onSuccess: () => {
              setSecretTarget(null);
              toast.push({
                tone: "success",
                title: t("pages.sso.providers.secretClearedToast", { name }),
              });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.saveFailed", { what: name }),
                detail: errorDetail(error),
              });
            },
          });
        }}
      />

      {/* out of service is reversible with one flip, so it confirms as a
          default-tone action; it turns destructive only when it would leave
          members no way in. the mutation is reset on close so a refusal for one
          provider does not greet the next */}
      <ConfirmDialog
        name="sso-provider-disable"
        open={!!disableTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDisableTarget(null);
          toggle.reset();
        }}
        title={t("pages.sso.disable.title", { name: disableTarget?.name })}
        description={t("pages.sso.disable.body")}
        confirmLabel={t("pages.sso.disable.confirm")}
        tone={disableLocksOut ? "danger" : "default"}
        pending={toggle.isPending}
        error={toggle.error}
        onConfirm={() => {
          if (!disableTarget) return;
          toggle.mutate(
            { provider: disableTarget, enabled: false },
            { onSuccess: () => setDisableTarget(null) },
          );
        }}
      >
        {disableLocksOut && disableTarget && <LockoutNotice name={disableTarget.name} />}
      </ConfirmDialog>

      <ConfirmDialog
        name="sso-connection-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.sso.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.sso.confirm.body")}
        confirmLabel={t("pages.sso.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorDetail(error),
              });
            },
          });
        }}
      >
        {deleteLocksOut && deleteTarget && <LockoutNotice name={deleteTarget.name} />}
      </ConfirmDialog>
    </PageBody>
  );
}
