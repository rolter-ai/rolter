import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { FieldError, describedBy } from "@/components/ui/field-error";
import { Textarea } from "@/components/ui/textarea";
import {
  fetchClusterNodes,
  fetchSecuritySettings,
  updateSecuritySettings,
  type SecuritySettingsDto,
  type UpdateSecuritySettingsInput,
} from "@/lib/api";
import { gatewayPickup, type Pickup } from "@/lib/gateway-pickup";
import { useFormat } from "@/lib/i18n/format";
import {
  entriesOf,
  listToText,
  normalizeRequiredHeaders,
  parseLists,
  problemCount,
  requiredHeadersPayload,
  requiredHeadersToText,
  type EntryProblem,
} from "@/lib/security-lists";
import { loosenings, type Loosening, type SecurityPolicy } from "@/lib/security-loosening";
import { errorDetail, useToast } from "@/lib/toast";
import { useDraft, type FieldEquality } from "@/lib/use-draft";
import { useScreenReady } from "@/lib/ux-react";

// every field holds one entry per line
interface FormState {
  allowedOrigins: string;
  allowedHeaders: string;
  requiredHeaders: string;
  bypassRoutes: string;
}

type ListKey = "allowedOrigins" | "allowedHeaders" | "requiredHeaders" | "bypassRoutes";

const fromDto = (dto: SecuritySettingsDto): FormState => ({
  allowedOrigins: listToText(dto.allowed_origins),
  allowedHeaders: listToText(dto.allowed_headers),
  requiredHeaders: requiredHeadersToText(dto.required_headers),
  bypassRoutes: listToText(dto.auth_bypass_routes),
});

// what the save sends, which is what "changed" has to mean: a blank line or the
// space around a colon is not an edit, and neither is a secret of spaces
const sameEntries = (a: string, b: string) => entriesOf(a).join("\n") === entriesOf(b).join("\n");
const EQUALS: FieldEquality<FormState> = {
  allowedOrigins: sameEntries,
  allowedHeaders: sameEntries,
  requiredHeaders: (a, b) => normalizeRequiredHeaders(a) === normalizeRequiredHeaders(b),
  bypassRoutes: sameEntries,
};

// the cards of the screen, by the fields each one holds: a card is marked when
// any of its fields changed, and the footer counts cards
const SECTIONS: (keyof FormState)[][] = [
  ["allowedOrigins"],
  ["allowedHeaders"],
  ["requiredHeaders"],
  ["bypassRoutes"],
];

const policyOf = (form: FormState): SecurityPolicy => ({
  authBypassRoutes: entriesOf(form.bypassRoutes),
});

// the request body, from a form whose lists all parse. Save is out of reach
// while one does not, so nothing is dropped on the way
function toInput(form: FormState): UpdateSecuritySettingsInput {
  const lists = parseLists(form);
  return {
    allowed_origins: lists.allowedOrigins.entries,
    allowed_headers: lists.allowedHeaders.entries,
    required_headers: requiredHeadersPayload(lists.requiredHeaders.entries),
    auth_bypass_routes: lists.bypassRoutes.entries,
  };
}

// a pasted list of two hundred bad lines should not be a wall of red
const MAX_PROBLEMS_SHOWN = 3;
// after a save, how often the gateways are asked whether they run it yet, and
// for how long. a gateway polls every few seconds, so a minute and a half with
// one still behind is a gateway that is not picking config up
const PICKUP_POLL_MS = 4_000;
const PICKUP_WATCH_MS = 90_000;

// global gateway security policy, persisted via /api/v1/security-settings
// (superadmin only). dashboard secret is write-only: the server seals it and
// reports only whether one is configured.
function SecurityScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const toast = useToast();
  const settings = useQuery({
    queryKey: ["security-settings"],
    queryFn: fetchSecuritySettings,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `settings` is the query the user is actually waiting on for this screen
  useScreenReady(!settings.isLoading);

  const source = React.useMemo(
    () => (settings.data ? fromDto(settings.data) : undefined),
    [settings.data],
  );
  const { draft: form, saved, changed, dirty, set, reset, commit } = useDraft(source, EQUALS);
  // a list is checked as it is typed, but its errors wait for the caret to
  // leave it: `h` on the way to `https://` is not a mistake yet. a problem the
  // form was loaded with is shown from the start, since nobody typed it
  const [touched, setTouched] = React.useState<Partial<Record<ListKey, boolean>>>({});
  // the loosenings a save is waiting on, kept after the dialog closes so its
  // body does not empty while it fades
  const [confirming, setConfirming] = React.useState<{ open: boolean; items: Loosening[] }>({
    open: false,
    items: [],
  });
  const [savedAt, setSavedAt] = React.useState<number | null>(null);

  const parsed = React.useMemo(() => (form ? parseLists(form) : null), [form]);
  const invalid = parsed ? problemCount(parsed) : 0;

  const save = useMutation({
    mutationFn: (f: FormState) => updateSecuritySettings(toInput(f)),
    onSuccess: (dto) => {
      queryClient.setQueryData(["security-settings"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["security-settings"] });
      commit(fromDto(dto));
      setTouched({});
      setConfirming((c) => ({ ...c, open: false }));
      setSavedAt(Date.now());
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.securitySettings") }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.securitySettings") }),
        detail: errorDetail(error),
      });
    },
  });

  // which gateways run the save. keyed on the save's own time so an inventory
  // read before it, from the Cluster screen or an earlier save, is never
  // mistaken for an answer to it
  const inventory = useQuery({
    queryKey: ["security-pickup", savedAt],
    queryFn: fetchClusterNodes,
    enabled: savedAt !== null,
    retry: false,
    refetchInterval: (query) => {
      const { state } = gatewayPickup(query.state.data, query.state.status === "error");
      const waiting = state === "checking" || state === "lagging";
      return waiting && savedAt !== null && Date.now() - savedAt < PICKUP_WATCH_MS
        ? PICKUP_POLL_MS
        : false;
    },
  });
  const pickup = gatewayPickup(inventory.data, inventory.isError);

  if (settings.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={3} height={148} />
      </div>
    );
  }
  if (settings.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={settings.error}
          resource={t("errors.resources.securitySettings")}
          onRetry={() => void settings.refetch()}
          target="security"
        />
      </div>
    );
  }
  if (!form || !saved || !parsed) return null;

  const isChanged = (keys: (keyof FormState)[]) => keys.some((key) => changed.includes(key));
  const changedCards = SECTIONS.filter(isChanged).length;

  const listProps = (key: ListKey, id: string, problems: EntryProblem[]) => ({
    id,
    value: form[key],
    changed: changed.includes(key),
    problems,
    // a problem in a field nobody has edited came from the store
    showProblems: touched[key] === true || !changed.includes(key),
    onChange: (value: string) => set({ [key]: value } as Pick<FormState, ListKey>),
    onBlur: () => setTouched((prev) => ({ ...prev, [key]: true })),
  });

  const discard = () => {
    reset();
    setTouched({});
    save.reset();
  };

  // a save that opens the gateway or the dashboard waits for a yes that lists
  // what it opens. a tightening, or an edit that changes nothing of the kind,
  // goes straight out: one request either way
  const requestSave = () => {
    if (invalid > 0) return;
    const opened = loosenings(policyOf(saved), policyOf(form));
    if (opened.length === 0) {
      save.mutate(form);
      return;
    }
    save.reset();
    setConfirming({ open: true, items: opened });
  };

  const statusTone =
    invalid > 0 ? "text-[color:var(--status-danger-text)]" : "text-[color:var(--text-subtle)]";
  let status: string | null = null;
  if (invalid > 0) {
    status = t("pages.security.status.invalid", { count: invalid });
  } else if (dirty) {
    status = t("pages.security.status.changed", { count: changedCards });
  } else if (savedAt !== null) {
    status = `${t("pages.security.status.saved", { time: fmt.time(savedAt) })} ${pickupLine(t, pickup)}`;
  }

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <ListCard
        title={t("pages.security.allowedOrigins")}
        desc={t("pages.security.allowedOriginsHint")}
        placeholder={t("pages.security.allowedOriginsPlaceholder")}
        {...listProps("allowedOrigins", "security-allowed-origins", parsed.allowedOrigins.problems)}
      />
      <ListCard
        title={t("pages.security.allowedHeaders")}
        desc={t("pages.security.allowedHeadersHint")}
        placeholder={t("pages.security.allowedHeadersPlaceholder")}
        {...listProps("allowedHeaders", "security-allowed-headers", parsed.allowedHeaders.problems)}
      />
      <ListCard
        title={t("pages.security.requiredHeaders")}
        desc={t("pages.security.requiredHeadersHint")}
        placeholder={t("pages.security.requiredHeadersPlaceholder")}
        {...listProps(
          "requiredHeaders",
          "security-required-headers",
          parsed.requiredHeaders.problems,
        )}
      />
      <ListCard
        title={t("pages.security.bypassRoutes")}
        desc={t("pages.security.bypassRoutesHint")}
        placeholder={t("pages.security.bypassRoutesPlaceholder")}
        {...listProps("bypassRoutes", "security-bypass-routes", parsed.bypassRoutes.problems)}
      />

      <div className="sticky bottom-0 flex flex-wrap items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {status && (
          <p
            id="security-save-status"
            className={`min-w-0 basis-full text-xs sm:mr-auto sm:basis-auto ${statusTone}`}
          >
            {status}
          </p>
        )}
        <Button variant="outline" disabled={!dirty || save.isPending} onClick={discard}>
          {t("pages.security.discard")}
        </Button>
        <Button
          disabled={!dirty || invalid > 0 || save.isPending}
          aria-describedby={status ? "security-save-status" : undefined}
          onClick={requestSave}
        >
          {save.isPending && !confirming.open ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>

      {/* mounted beside the form, not inside it, so it can report the landing
          on the render that closes it */}
      <ConfirmDialog
        name="security-loosen"
        open={confirming.open}
        onOpenChange={(open) => {
          if (open) return;
          setConfirming((c) => ({ ...c, open: false }));
          // an error from a refused save would otherwise greet the next attempt
          save.reset();
        }}
        title={t("pages.security.confirm.title", { count: confirming.items.length })}
        description={t("pages.security.confirm.body", { count: confirming.items.length })}
        confirmLabel={t("pages.security.confirm.confirm")}
        tone="default"
        pending={save.isPending}
        error={save.error}
        onConfirm={() => save.mutate(form)}
      >
        <ul className="flex flex-col gap-2.5">
          {confirming.items.map((item) => (
            <LooseningRow key={item.route} item={item} />
          ))}
        </ul>
      </ConfirmDialog>
    </div>
  );
}

// what the gateways say about the save just made, in words that are true of
// each answer: only a gateway the control plane has heard from can be counted
function pickupLine(t: TFunction, pickup: Pickup): string {
  switch (pickup.state) {
    case "checking":
      return t("pages.security.pickup.checking");
    case "unavailable":
      return t("pages.security.pickup.unavailable");
    case "none":
      return t("pages.security.pickup.none");
    case "converged":
      return t("pages.security.pickup.converged", { count: pickup.total });
    case "lagging":
      return t("pages.security.pickup.lagging", {
        count: pickup.total,
        converged: pickup.converged,
      });
  }
}

function ChangedBadge() {
  const { t } = useTranslation();
  return <Badge tone="warning">{t("pages.security.edited")}</Badge>;
}

// one line of the confirmation: what opens, and what that means
function LooseningRow({ item }: { item: Loosening }) {
  const { t } = useTranslation();
  return (
    <li className="flex flex-col gap-0.5">
      <span className="text-sm font-medium">
        {t(`pages.security.confirm.${item.kind}.label`)}{" "}
        <code className="font-mono text-xs">{item.route}</code>
      </span>
      <span className="text-sm text-muted-foreground">
        {t(`pages.security.confirm.${item.kind}.effect`)}
      </span>
    </li>
  );
}

// a list edited one entry per line. an entry that does not parse stays in the
// field and is named under it, with the control pointing at the reason
function ListCard({
  id,
  title,
  desc,
  value,
  placeholder,
  changed,
  problems,
  showProblems,
  onChange,
  onBlur,
}: {
  id: string;
  title: string;
  desc: string;
  value: string;
  placeholder: string;
  changed: boolean;
  problems: EntryProblem[];
  showProblems: boolean;
  onChange: (v: string) => void;
  onBlur: () => void;
}) {
  const { t } = useTranslation();
  const hintId = `${id}-hint`;
  const shown = showProblems ? problems : [];
  const visible = shown.slice(0, MAX_PROBLEMS_SHOWN);
  const hidden = shown.length - visible.length;
  const errorId = (index: number) => `${id}-error-${index}`;
  const errorIds = Array.from({ length: visible.length + (hidden > 0 ? 1 : 0) }, (_, i) =>
    errorId(i),
  );
  return (
    <section className="flex flex-col gap-2.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div>
        <div className="flex items-center gap-2">
          <label htmlFor={id} className="text-sm font-medium">
            {title}
          </label>
          {changed && <ChangedBadge />}
        </div>
        <p id={hintId} className="mt-1 text-sm text-muted-foreground">
          {desc}
        </p>
      </div>
      <Textarea
        id={id}
        className="min-h-[76px] font-mono text-xs"
        // one entry per line, so the field grows with its list instead of
        // scrolling the third entry out of sight
        rows={Math.min(8, Math.max(3, value.split("\n").length + 1))}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-invalid={shown.length > 0 || undefined}
        aria-describedby={describedBy(hintId, ...errorIds)}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
      />
      {visible.map((problem, index) => (
        <FieldError
          key={`${problem.line}-${problem.code}`}
          id={errorId(index)}
          error={t(`pages.security.errors.${problem.code}`, {
            line: problem.line,
            entry: problem.entry,
          })}
        />
      ))}
      {hidden > 0 && (
        <FieldError
          id={errorId(visible.length)}
          error={t("pages.security.errors.more", { count: hidden })}
        />
      )}
    </section>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(SecurityScreen, "errors.resources.securitySettings");
