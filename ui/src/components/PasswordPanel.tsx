import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { PanelSkeleton } from "@/components/LoadingState";
import { ME_KEY } from "@/components/ProfileCard";
import { Field } from "@/components/ui/field";
import { FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import {
  ApiError,
  MIN_PASSWORD_LEN,
  changeMyPassword,
  fetchMe,
  isOpenModeNoSession,
} from "@/lib/api";
import { describeError } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";

/** a password is a short string: the inputs stay a column while the messages below run wider */
const FIELD_WIDTH = "max-w-sm";

/** where a refusal is shown: on one of the three inputs, or on the form as a whole */
interface Problems {
  current?: string;
  next?: string;
  confirm?: string;
  form?: string;
}

/**
 * The account's own password (#2804).
 *
 * Self-service like the profile and the second factor beside it: the route
 * writes only the caller's own row, so `my_password:update` is open to every
 * signed-in account and the button asks for no role. What the server decides
 * is the rest. A wrong current password is a `400` on that field, counted
 * against the same budget as a sign-in, so a spent budget answers `429` with
 * the wait; every other session of the account ends and the answer says how
 * many, while this one stays signed in.
 *
 * An account that signs in through single sign-on has no local password
 * (`has_local_password: false` on `/auth/me`), so it is told why instead of
 * being offered a form that can only answer `409 no_local_password`. A read of
 * `/auth/me` that fails leaves the form up: the route decides for itself, and
 * the profile card above already reports the failed read.
 *
 * Passwords are never trimmed, logged or kept past a successful change: they
 * live in this component's state and the request body only.
 */
export function PasswordPanel() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();

  const me = useQuery({ queryKey: [ME_KEY], queryFn: fetchMe, retry: false });

  const [current, setCurrent] = React.useState("");
  const [next, setNext] = React.useState("");
  const [confirm, setConfirm] = React.useState("");
  const [problems, setProblems] = React.useState<Problems>({});
  // how many other sessions the last change ended; `null` until one succeeds
  const [revoked, setRevoked] = React.useState<number | null>(null);

  const currentRef = React.useRef<HTMLInputElement>(null);
  const nextRef = React.useRef<HTMLInputElement>(null);
  const confirmRef = React.useRef<HTMLInputElement>(null);

  // the first input that carries a problem takes focus where the problem is
  // set, not from an effect on the problems: editing one field clears only its
  // own message, and an effect would drag the caret to whichever other field
  // still had one while the person is typing
  const fail = (found: Problems) => {
    setProblems(found);
    const target = found.current
      ? currentRef
      : found.next
        ? nextRef
        : found.confirm
          ? confirmRef
          : null;
    target?.current?.focus();
  };

  /** what the form can tell is wrong before asking the server */
  const check = (): Problems => {
    const found: Problems = {};
    if (current === "") found.current = t("account.password.problem.currentEmpty");
    if ([...next].length < MIN_PASSWORD_LEN) {
      found.next = t("account.password.problem.tooShort", { min: MIN_PASSWORD_LEN });
    } else if (current !== "" && next === current) {
      found.next = t("account.password.problem.same");
    }
    if (confirm !== next) found.confirm = t("account.password.problem.mismatch");
    return found;
  };

  /** the server's refusal, placed on the input it names */
  const refusal = (error: unknown): Problems => {
    if (error instanceof ApiError) {
      if (error.status === 400 && error.code === "invalid_field") {
        if (error.field === "current_password") {
          return { current: t("account.password.problem.currentWrong") };
        }
        if (error.field === "new_password") {
          // the server answers one code for both rules; the form knows which
          // one the draft breaks, and falls back to a plain refusal for any
          // other the control plane may add
          if ([...next].length < MIN_PASSWORD_LEN) {
            return { next: t("account.password.problem.tooShort", { min: MIN_PASSWORD_LEN }) };
          }
          if (next === current) return { next: t("account.password.problem.same") };
          return { next: t("account.password.problem.rejected") };
        }
      }
      if (error.status === 429) {
        const wait = error.retryAfterSeconds && Math.ceil(error.retryAfterSeconds);
        return {
          form: wait
            ? t("account.password.problem.throttled", { count: wait, wait: fmt.number(wait) })
            : t("account.password.problem.throttledNoWait"),
        };
      }
    }
    // everything else, a 409 no_local_password included, reads from the shared
    // error copy (#2841)
    const copy = describeError(error, t);
    return {
      form: [t("account.password.problem.failed"), copy.message, copy.detail]
        .filter(Boolean)
        .join(" "),
    };
  };

  const change = useMutation({
    mutationFn: () => changeMyPassword({ current_password: current, new_password: next }),
    onSuccess: (result) => {
      setCurrent("");
      setNext("");
      setConfirm("");
      setProblems({});
      setRevoked(result.sessions_revoked);
    },
    onError: (error) => {
      setRevoked(null);
      fail(refusal(error));
      // a 409 means the form was offered to an account that has no local
      // password, so the read it was drawn from is out of date
      if (error instanceof ApiError && error.code === "no_local_password") {
        void queryClient.invalidateQueries({ queryKey: [ME_KEY] });
      }
    },
  });

  // open mode has no accounts, so there is no password to change, and the keys
  // panel already says why the screen is inert; an older control plane that
  // does not mount /auth/me answers 404 for the same reason (#942)
  if (isOpenModeNoSession(me.error)) return null;
  if (me.error instanceof ApiError && me.error.status === 404) return null;

  if (me.isLoading) {
    return (
      <SettingsPanel title={t("account.password.title")}>
        <PanelSkeleton panels={1} height={200} className="w-full" />
      </SettingsPanel>
    );
  }

  // an account with no local password has nothing to change here; `undefined`
  // is a control plane that predates the field, which reads as "has one"
  if (me.data?.has_local_password === false) {
    return (
      <SettingsPanel
        title={t("account.password.title")}
        description={t("account.password.ssoOnly")}
      />
    );
  }

  // typing starts the next attempt: the old success and the field's own
  // refusal are about a draft that no longer exists
  const edit =
    (set: (value: string) => void, field: "current" | "next" | "confirm") =>
    (event: React.ChangeEvent<HTMLInputElement>) => {
      set(event.target.value);
      setRevoked(null);
      setProblems((found) =>
        found[field] || found.form ? { ...found, [field]: undefined, form: undefined } : found,
      );
    };

  return (
    <SettingsPanel title={t("account.password.title")} description={t("account.password.subtitle")}>
      <form
        className="flex w-full flex-col gap-3.5"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          if (change.isPending) return;
          const found = check();
          if (Object.keys(found).length > 0) {
            fail(found);
            return;
          }
          setProblems({});
          change.mutate();
        }}
      >
        <Field
          label={t("account.password.current")}
          error={problems.current}
          className={FIELD_WIDTH}
        >
          <Input
            ref={currentRef}
            type="password"
            value={current}
            autoComplete="current-password"
            onChange={edit(setCurrent, "current")}
          />
        </Field>
        <Field
          label={t("account.password.new")}
          className={FIELD_WIDTH}
          error={problems.next}
          hint={t("account.password.newHint", { min: MIN_PASSWORD_LEN })}
        >
          <Input
            ref={nextRef}
            type="password"
            value={next}
            autoComplete="new-password"
            onChange={edit(setNext, "next")}
          />
        </Field>
        <Field
          label={t("account.password.confirm")}
          error={problems.confirm}
          className={FIELD_WIDTH}
        >
          <Input
            ref={confirmRef}
            type="password"
            value={confirm}
            autoComplete="new-password"
            onChange={edit(setConfirm, "confirm")}
          />
        </Field>
        {problems.form && (
          <div role="alert">
            <FieldError id="password-form-error" error={problems.form} />
          </div>
        )}
        {revoked !== null && (
          <p
            role="status"
            className="flex items-start gap-2 text-sm text-[color:var(--status-success-text)]"
          >
            <Check aria-hidden className="mt-0.5 h-4 w-4 flex-none" />
            <span>
              {revoked === 0
                ? t("account.password.changedAlone")
                : t("account.password.changed", { count: revoked, sessions: fmt.number(revoked) })}
            </span>
          </p>
        )}
        <div>
          {/* `my_password:update` is open to every signed-in account, so this
              never refuses a real caller; the gate keeps the control on the
              same table every other control reads, should the rule ever tighten */}
          <GatedButton
            gate="my_password:update"
            control="account-password-change"
            type="submit"
            size="sm"
            disabled={change.isPending}
          >
            {t("account.password.submit")}
          </GatedButton>
        </div>
      </form>
    </SettingsPanel>
  );
}
