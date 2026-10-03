import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import {
  fetchCompatibilityPolicy,
  updateCompatibilityPolicy,
  type CompatibilityPolicyDto,
} from "@/lib/api";
import { serverFieldError } from "@/lib/field-errors";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

interface FormState {
  anthropicVersion: string;
  defaultMaxTokens: string;
}

const fromDto = (dto: CompatibilityPolicyDto): FormState => ({
  anthropicVersion: dto.anthropic_version,
  defaultMaxTokens: String(dto.default_max_tokens),
});

// a dated Anthropic release string like 2023-06-01. the gateway forwards this
// verbatim as anthropic-version, so a malformed value fails every Anthropic
// call at the edge with an opaque upstream error
const DATED_RELEASE = /^\d{4}-\d{2}-\d{2}$/;

// mirrors the server's validation so a bad value is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// it names a catalog key rather than carrying english copy — the screen renders
// it, which is where `t` lives
// every failing field is reported at once, in form order, keyed by field (#2096)
type FieldErrors = Partial<Record<keyof FormState, string>>;

function validate(form: FormState): FieldErrors {
  const errors: FieldErrors = {};
  if (!DATED_RELEASE.test(form.anthropicVersion.trim())) {
    errors.anthropicVersion = "pages.compatibility.validation.anthropicVersion";
  }
  const tokens = Number(form.defaultMaxTokens);
  if (!Number.isInteger(tokens) || tokens < 1 || tokens > 1_000_000) {
    errors.defaultMaxTokens = "pages.compatibility.validation.defaultMaxTokens";
  }
  return errors;
}

// the order the fields sit in, so focus lands on the first one that is wrong
const FIELD_ORDER: (keyof FormState)[] = ["anthropicVersion", "defaultMaxTokens"];

// the wire names a 400 opens with, mapped to the field they belong to
const WIRE_FIELDS: Record<string, keyof FormState> = {
  anthropic_version: "anthropicVersion",
  default_max_tokens: "defaultMaxTokens",
};

// cross-dialect compatibility policy, persisted via /api/v1/compatibility-policy
// (superadmin only). these are the values applied when a request is translated
// between the OpenAI and Anthropic wire formats (#546)
function CompatibilityScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const policy = useQuery({
    queryKey: ["compatibility-policy"],
    queryFn: fetchCompatibilityPolicy,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `policy` is the query the user is actually waiting on for this screen
  useScreenReady(!policy.isLoading);
  useErrorState(!!policy.error, "compatibility");

  const [form, setForm] = React.useState<FormState | null>(null);
  React.useEffect(() => {
    if (policy.data && form === null) {
      setForm(fromDto(policy.data));
    }
  }, [policy.data, form]);

  const [serverErrors, setServerErrors] = React.useState<FieldErrors>({});
  const versionId = React.useId();
  const tokensId = React.useId();
  const ids: Record<keyof FormState, string> = {
    anthropicVersion: versionId,
    defaultMaxTokens: tokensId,
  };

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateCompatibilityPolicy({
        anthropic_version: f.anthropicVersion.trim(),
        default_max_tokens: Number(f.defaultMaxTokens),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["compatibility-policy"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["compatibility-policy"] });
      setForm(fromDto(dto));
      setServerErrors({});
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.compatibilitySettings") }),
      });
    },
    onError: (error) => {
      const named = serverFieldError(error, WIRE_FIELDS);
      if (named) {
        setServerErrors({ [named.field]: named.message });
        document.getElementById(ids[named.field])?.focus();
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.compatibilitySettings") }),
        detail: errorDetail(error),
      });
    },
  });

  if (policy.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={2} height={112} />
      </div>
    );
  }
  if (policy.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={policy.error}
          resource={t("errors.resources.compatibilitySettings")}
          onRetry={() => void policy.refetch()}
        />
      </div>
    );
  }
  if (!form) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    // an edit answers the server's complaint about that field
    setServerErrors((e) => {
      const next = { ...e };
      for (const key of Object.keys(patch) as (keyof FormState)[]) delete next[key];
      return next;
    });
  };
  const local = validate(form);
  const errors: Record<keyof FormState, string | undefined> = {
    anthropicVersion: local.anthropicVersion
      ? t(local.anthropicVersion)
      : serverErrors.anthropicVersion,
    defaultMaxTokens: local.defaultMaxTokens
      ? t(local.defaultMaxTokens)
      : serverErrors.defaultMaxTokens,
  };
  const invalid = FIELD_ORDER.filter((key) => errors[key]);
  // Save stays pressable while the form is invalid so a press can say why: it
  // moves focus to the first field at fault rather than doing nothing (#2096)
  const submit = () => {
    if (invalid.length > 0) {
      document.getElementById(ids[invalid[0]])?.focus();
      return;
    }
    save.mutate(form);
  };
  // the server owns this list, so the screen warns without knowing which
  // fields need a restart
  const restartRequired = policy.data?.restart_required ?? [];

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.compatibility.version.title")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.compatibility.version.desc"
              components={[<code key="header" className="font-mono text-xs" />]}
            />
          </p>
        </div>
        <Input
          className="max-w-[200px] font-mono text-xs"
          id={versionId}
          aria-label={t("pages.compatibility.version.aria")}
          aria-invalid={errors.anthropicVersion ? true : undefined}
          aria-describedby={describedBy(!!errors.anthropicVersion && `${versionId}-error`)}
          placeholder="2023-06-01"
          value={form.anthropicVersion}
          onChange={(e) => set({ anthropicVersion: e.target.value })}
        />
        <FieldError id={`${versionId}-error`} error={errors.anthropicVersion} />
      </section>

      <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.compatibility.maxTokens.title")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.compatibility.maxTokens.desc"
              components={[<code key="field" className="font-mono text-xs" />]}
            />
          </p>
        </div>
        <Input
          className="max-w-[200px]"
          inputMode="numeric"
          id={tokensId}
          aria-label={t("pages.compatibility.maxTokens.aria")}
          aria-invalid={errors.defaultMaxTokens ? true : undefined}
          aria-describedby={describedBy(!!errors.defaultMaxTokens && `${tokensId}-error`)}
          value={form.defaultMaxTokens}
          onChange={(e) => set({ defaultMaxTokens: e.target.value })}
        />
        <FieldError id={`${tokensId}-error`} error={errors.defaultMaxTokens} />
      </section>

      {restartRequired.length > 0 && (
        <section className="flex items-start gap-3 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
          <Badge tone="warning">RESTART</Badge>
          <p className="text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.compatibility.restartRequired"
              values={{ fields: restartRequired.join(", ") }}
              components={[<span key="fields" className="font-mono text-xs" />]}
            />
          </p>
        </section>
      )}

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {invalid.length > 0 && (
          <span role="status" className="text-xs text-[color:var(--status-danger-text)]">
            {t("common.fieldsNeedAttention", { count: invalid.length })}
          </span>
        )}
        <Button
          disabled={save.isPending}
          aria-disabled={invalid.length > 0 || undefined}
          className={invalid.length > 0 ? "opacity-50" : undefined}
          onClick={submit}
        >
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(CompatibilityScreen, "errors.resources.compatibilitySettings");
