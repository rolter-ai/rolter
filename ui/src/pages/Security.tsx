import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { SettingsSaveBar } from "@/components/SettingsSaveBar";
import { Badge } from "@/components/ui/badge";
import { FieldError, describedBy } from "@/components/ui/field-error";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Textarea } from "@/components/ui/textarea";
import {
  fetchSecuritySettings,
  updateSecuritySettings,
  type SecuritySettingsDto,
  type UpdateSecuritySettingsInput,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useErrorVisibility } from "@/lib/error-visibility";
import {
  entriesOf,
  listToText,
  normalizeRequiredHeaders,
  parseLists,
  parseRequiredHeaders,
  requiredHeadersPayload,
  requiredHeadersToText,
  type EntryProblem,
} from "@/lib/security-lists";
import {
  loosenings,
  looseningSubject,
  type Loosening,
  type SecurityPolicy,
} from "@/lib/security-loosening";
import { useToast } from "@/lib/toast";
import type { FieldEquality } from "@/lib/use-draft";
import { useSettingsForm } from "@/lib/use-settings-form";
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

// the lists in the order they sit on the screen, so focus lands on the first
// one at fault, and the control each one's focus goes to
const ORDER: readonly ListKey[] = [
  "allowedOrigins",
  "allowedHeaders",
  "requiredHeaders",
  "bypassRoutes",
];
const CONTROL_ID: Record<ListKey, string> = {
  allowedOrigins: "security-allowed-origins",
  allowedHeaders: "security-allowed-headers",
  requiredHeaders: "security-required-headers",
  bypassRoutes: "security-bypass-routes",
};

const policyOf = (form: FormState): SecurityPolicy => ({
  authBypassRoutes: entriesOf(form.bypassRoutes),
  // a line that does not parse blocks the save, so only the headers that do count
  requiredHeaders: parseRequiredHeaders(form.requiredHeaders).entries.map((entry) => entry.name),
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
// global gateway security policy, persisted via /api/v1/security-settings
// (superadmin only).
function SecurityScreen() {
  const { t } = useTranslation();
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
  // a list is checked as it is typed, but its errors wait for the caret to
  // leave it: `h` on the way to `https://` is not a mistake yet. a problem the
  // form was loaded with is shown from the start, since nobody typed it
  const visibility = useErrorVisibility<ListKey>();
  const sf = useSettingsForm<FormState, ListKey>({
    source,
    equals: EQUALS,
    fieldErrors: (draft) => {
      const lists = draft ? parseLists(draft) : null;
      const first = (key: ListKey) => {
        const problem = lists?.[key].problems[0];
        return problem
          ? t(`pages.security.errors.${problem.code}`, { line: problem.line, entry: problem.entry })
          : undefined;
      };
      return {
        order: ORDER,
        wire: {},
        idOf: (key) => CONTROL_ID[key],
        local: Object.fromEntries(ORDER.map((key) => [key, first(key)])),
        // a problem in a list nobody has edited came from the store
        shows: (key) =>
          visibility.shows(key) || !draft || !source || EQUALS[key]!(draft[key], source[key]),
      };
    },
  });
  const { draft: form, saved, changed, set } = sf;
  // the loosenings a save is waiting on, kept after the dialog closes so its
  // body does not empty while it fades
  const [confirming, setConfirming] = React.useState<{ open: boolean; items: Loosening[] }>({
    open: false,
    items: [],
  });

  const parsed = React.useMemo(() => (form ? parseLists(form) : null), [form]);

  const save = useMutation({
    mutationFn: (f: FormState) => updateSecuritySettings(toInput(f)),
    onSuccess: (dto) => {
      queryClient.setQueryData(["security-settings"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["security-settings"] });
      sf.commit(fromDto(dto));
      visibility.reset();
      setConfirming((c) => ({ ...c, open: false }));
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
        detail: errorText(error, t),
      });
    },
  });

  if (settings.isLoading) {
    return (
      <SettingsBody>
        <PanelSkeleton panels={3} height={148} />
      </SettingsBody>
    );
  }
  if (settings.isError) {
    return (
      <SettingsBody>
        <LoadError
          error={settings.error}
          resource={t("errors.resources.securitySettings")}
          onRetry={() => void settings.refetch()}
          target="security"
        />
      </SettingsBody>
    );
  }
  if (!form || !saved || !parsed) return null;

  const listProps = (key: ListKey, id: string, problems: EntryProblem[]) => ({
    id,
    value: form[key],
    changed: changed.includes(key),
    problems,
    // a problem in a field nobody has edited came from the store
    showProblems: visibility.shows(key) || !changed.includes(key),
    onChange: (value: string) => set({ [key]: value } as Pick<FormState, ListKey>),
    onBlur: () => visibility.touch(key),
  });

  const discard = () => {
    sf.discard();
    visibility.reset();
    save.reset();
  };

  // a save that opens the gateway or the dashboard waits for a yes that lists
  // what it opens. a tightening, or an edit that changes nothing of the kind,
  // goes straight out: one request either way
  const requestSave = () => {
    const opened = loosenings(policyOf(saved), policyOf(form));
    if (opened.length === 0) {
      save.mutate(form);
      return;
    }
    save.reset();
    setConfirming({ open: true, items: opened });
  };

  return (
    <SettingsBody onSubmit={() => sf.fields.submit(requestSave)}>
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

      <SettingsSaveBar
        form={{ ...sf, discard }}
        pending={save.isPending && !confirming.open}
        updatedAt={settings.data?.updated_at}
      />

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
            <LooseningRow key={`${item.kind}:${looseningSubject(item)}`} item={item} />
          ))}
        </ul>
      </ConfirmDialog>
    </SettingsBody>
  );
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
        <code className="font-mono text-xs">{looseningSubject(item)}</code>
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
    <SettingsPanel
      title={title}
      description={<span id={hintId}>{desc}</span>}
      badge={changed && <ChangedBadge />}
      className="gap-2.5"
    >
      <Textarea
        id={id}
        aria-label={title}
        className="min-h-[76px] w-full font-mono text-xs"
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
    </SettingsPanel>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  SecurityScreen,
  "errors.resources.securitySettings",
  "security_settings:read",
);
