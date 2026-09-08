import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  fetchCompatibilityPolicy,
  updateCompatibilityPolicy,
  type CompatibilityPolicyDto,
} from "@/lib/api";
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
function validate(form: FormState): string | null {
  if (!DATED_RELEASE.test(form.anthropicVersion.trim())) {
    return "pages.compatibility.validation.anthropicVersion";
  }
  const tokens = Number(form.defaultMaxTokens);
  if (!Number.isInteger(tokens) || tokens < 1 || tokens > 1_000_000) {
    return "pages.compatibility.validation.defaultMaxTokens";
  }
  return null;
}

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
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.compatibilitySettings") }),
      });
    },
    onError: (error) => {
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
  };
  const localErrorKey = validate(form);
  const localError = localErrorKey ? t(localErrorKey) : null;
  // the server owns this list, so the screen warns without knowing which
  // fields need a restart
  const restartRequired = policy.data?.restart_required ?? [];

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">
            {t("pages.compatibility.version.title")}
          </span>
          <p className="mt-1 text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.compatibility.version.desc"
              components={[<code key="header" className="font-mono text-xs" />]}
            />
          </p>
        </div>
        <Input
          className="max-w-[200px] font-mono text-xs"
          aria-label={t("pages.compatibility.version.aria")}
          placeholder="2023-06-01"
          value={form.anthropicVersion}
          onChange={(e) => set({ anthropicVersion: e.target.value })}
        />
      </section>

      <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">
            {t("pages.compatibility.maxTokens.title")}
          </span>
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
          aria-label={t("pages.compatibility.maxTokens.aria")}
          value={form.defaultMaxTokens}
          onChange={(e) => set({ defaultMaxTokens: e.target.value })}
        />
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
        {localError && <span className="text-xs text-[color:var(--status-danger-text)]">{localError}</span>}
        <Button
          disabled={save.isPending || localError !== null}
          onClick={() => save.mutate(form)}
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
