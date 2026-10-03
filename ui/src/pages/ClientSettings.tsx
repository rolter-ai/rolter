import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatewayBasePrompt } from "@/components/GatewayBasePrompt";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { fetchClientSettings, updateClientSettings, type ClientSettingsDto } from "@/lib/api";
import { serverFieldError } from "@/lib/field-errors";
import { gatewayBase } from "@/lib/gateway";
import { errorDetail, useToast } from "@/lib/toast";
import { CLIENT_SETTINGS_QUERY_KEY } from "@/lib/use-gateway-base";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// injected headers are edited as an ordered list rather than an object so a
// half-typed name does not collide with an existing key while it is being typed
interface HeaderPair {
  id: number;
  name: string;
  value: string;
}

interface FormState {
  publicBaseUrl: string;
  forwarded: string;
  injected: HeaderPair[];
  requestIdHeader: string;
}

let nextPairId = 0;
// a shape for the field, not an address the dashboard hands out
const BASE_URL_PLACEHOLDER = "https://gateway.example.com";

const pair = (name = "", value = ""): HeaderPair => ({ id: nextPairId++, name, value });

// the allowlist is a comma or newline separated list in the textbox; both are
// natural to paste, and neither is legal inside a header name
const splitList = (raw: string) =>
  raw
    .split(/[\n,]/)
    .map((h) => h.trim().toLowerCase())
    .filter((h) => h.length > 0);

const fromDto = (dto: ClientSettingsDto): FormState => ({
  publicBaseUrl: dto.public_base_url ?? "",
  forwarded: dto.forwarded_headers.join(", "),
  injected: Object.entries(dto.injected_headers).map(([name, value]) => pair(name, value)),
  requestIdHeader: dto.request_id_header,
});

// rfc 9110 token characters; the server enforces the same shape, so a typo is
// caught here rather than after a round trip
const HEADER_NAME = /^[A-Za-z0-9!#$%&'*+.^_`|~-]+$/;

// a catalog key and the values it interpolates; the screen renders it, which
// is where `t` lives
interface FormError {
  key: string;
  values?: Record<string, string>;
}

// every failing field is reported at once, keyed by field (#2651). an injected
// header row contributes two fields, its name and its value, keyed by row id
type FieldKey = string;
type FieldErrors = Record<FieldKey, FormError>;

const nameKey = (row: HeaderPair) => `injected-name-${row.id}`;
const valueKey = (row: HeaderPair) => `injected-value-${row.id}`;

// the order the fields sit in, so focus lands on the first one that is wrong
const fieldOrder = (form: FormState): FieldKey[] => [
  "publicBaseUrl",
  "forwarded",
  ...form.injected.flatMap((row) => [nameKey(row), valueKey(row)]),
  "requestIdHeader",
];

// the wire names a 400 opens with, mapped to the field they belong to; the
// header rules answer with the header itself, which belongs to no one field
const WIRE_FIELDS: Record<string, FieldKey> = {
  public_base_url: "publicBaseUrl",
};

function validate(form: FormState, reserved: string[]): FieldErrors {
  const errors: FieldErrors = {};
  const url = form.publicBaseUrl.trim();
  if (url !== "" && !/^https?:\/\//.test(url)) {
    errors.publicBaseUrl = { key: "pages.clientSettings.errors.baseUrlScheme" };
  }
  const forwarded = splitList(form.forwarded);
  for (const name of forwarded) {
    if (!HEADER_NAME.test(name)) {
      errors.forwarded = { key: "pages.clientSettings.errors.badHeaderName", values: { name } };
      break;
    }
    if (reserved.includes(name)) {
      errors.forwarded = { key: "pages.clientSettings.errors.reservedHeader", values: { name } };
      break;
    }
  }
  if (!errors.forwarded && forwarded.length > 64) {
    errors.forwarded = { key: "pages.clientSettings.errors.tooManyForwarded" };
  }

  const seen = new Set<string>();
  let lastNamed: HeaderPair | null = null;
  for (const row of form.injected) {
    const { name, value } = row;
    const lower = name.trim().toLowerCase();
    if (lower === "" && value.trim() === "") continue;
    lastNamed = row;
    if (!HEADER_NAME.test(lower)) {
      errors[nameKey(row)] = { key: "pages.clientSettings.errors.badHeaderName", values: { name } };
    } else if (reserved.includes(lower)) {
      errors[nameKey(row)] = {
        key: "pages.clientSettings.errors.reservedHeader",
        values: { name: lower },
      };
    } else if (seen.has(lower)) {
      errors[nameKey(row)] = {
        key: "pages.clientSettings.errors.duplicateHeader",
        values: { name: lower },
      };
    }
    seen.add(lower);
    if (/[\u0000-\u001f\u007f]/.test(value)) {
      errors[valueKey(row)] = {
        key: "pages.clientSettings.errors.controlCharacters",
        values: { name: lower },
      };
    }
  }
  // the limit belongs to the row that crossed it
  if (seen.size > 64 && lastNamed && !errors[nameKey(lastNamed)]) {
    errors[nameKey(lastNamed)] = { key: "pages.clientSettings.errors.tooManyInjected" };
  }

  const requestId = form.requestIdHeader.trim().toLowerCase();
  if (!HEADER_NAME.test(requestId)) {
    errors.requestIdHeader = { key: "pages.clientSettings.errors.badRequestIdHeader" };
  } else if (reserved.includes(requestId)) {
    errors.requestIdHeader = {
      key: "pages.clientSettings.errors.reservedHeader",
      values: { name: requestId },
    };
  }
  return errors;
}

// deployment-wide client settings, persisted via /api/v1/client-settings
// (superadmin only): the base URL the dashboard hands out, and what the gateway
// does with headers on the upstream leg
function ClientSettingsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const settings = useQuery({
    queryKey: CLIENT_SETTINGS_QUERY_KEY,
    queryFn: fetchClientSettings,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `settings` is the query the user is actually waiting on for this screen
  useScreenReady(!settings.isLoading);
  useErrorState(!!settings.error, "client-settings");

  const [form, setForm] = React.useState<FormState | null>(null);
  const [serverErrors, setServerErrors] = React.useState<Record<FieldKey, string>>({});
  const base = React.useId();
  const idOf = (key: FieldKey) => {
    if (key === "publicBaseUrl") return "client-public-base-url";
    if (key === "requestIdHeader") return "client-request-id-header";
    return `${base}-${key}`;
  };
  React.useEffect(() => {
    if (settings.data && form === null) {
      setForm(fromDto(settings.data));
    }
  }, [settings.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateClientSettings({
        public_base_url: f.publicBaseUrl.trim() === "" ? null : f.publicBaseUrl.trim(),
        forwarded_headers: splitList(f.forwarded),
        injected_headers: Object.fromEntries(
          f.injected
            .map(({ name, value }) => [name.trim().toLowerCase(), value] as const)
            .filter(([name]) => name !== ""),
        ),
        request_id_header: f.requestIdHeader.trim().toLowerCase(),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(CLIENT_SETTINGS_QUERY_KEY, dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: CLIENT_SETTINGS_QUERY_KEY });
      setForm(fromDto(dto));
      setServerErrors({});
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.clientSettings") }),
      });
    },
    onError: (error) => {
      const named = serverFieldError(error, WIRE_FIELDS);
      if (named) {
        setServerErrors({ [named.field]: named.message });
        document.getElementById(idOf(named.field))?.focus();
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.clientSettings") }),
        detail: errorDetail(error),
      });
    },
  });

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
          resource={t("errors.resources.clientSettings")}
          onRetry={() => void settings.refetch()}
        />
      </div>
    );
  }
  if (!form || !settings.data) return null;

  const dto = settings.data;
  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };
  // an edit answers the server's complaint about that field
  const edit = (key: FieldKey, patch: Partial<FormState>) => {
    set(patch);
    setServerErrors((e) => {
      const next = { ...e };
      delete next[key];
      return next;
    });
  };
  const local = validate(form, dto.reserved);
  const errorOf = (key: FieldKey): string | undefined => {
    const found = local[key];
    return found ? t(found.key, found.values) : serverErrors[key];
  };
  const invalid = fieldOrder(form).filter((key) => errorOf(key));
  // Save stays pressable while the form is invalid so a press can say why: it
  // moves focus to the first field at fault rather than doing nothing (#2651)
  const submit = () => {
    if (invalid.length > 0) {
      document.getElementById(idOf(invalid[0]))?.focus();
      return;
    }
    save.mutate(form);
  };
  const errorId = (key: FieldKey) => `${idOf(key)}-error`;
  const invalidProps = (key: FieldKey) => ({
    id: idOf(key),
    "aria-invalid": errorOf(key) ? (true as const) : undefined,
    "aria-describedby": describedBy(!!errorOf(key) && errorId(key)),
  });
  // what a client would actually type, following the field as it is edited.
  // an empty field has no example: the /gw proxy needs a dashboard session, so
  // it is no address for a client (#2486)
  const exampleBase = gatewayBase(form.publicBaseUrl)?.url ?? null;

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.clientSettings.baseUrl")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.clientSettings.baseUrlHint")}
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="client-public-base-url"
            className="text-xs font-medium text-[color:var(--text-secondary)]"
          >
            {t("pages.clientSettings.publicBaseUrl")}
          </label>
          <Input
            className="min-w-[320px] font-mono text-xs"
            aria-label={t("pages.clientSettings.publicBaseUrl")}
            {...invalidProps("publicBaseUrl")}
            placeholder={BASE_URL_PLACEHOLDER}
            value={form.publicBaseUrl}
            onChange={(e) => edit("publicBaseUrl", { publicBaseUrl: e.target.value })}
          />
          <FieldError id={errorId("publicBaseUrl")} error={errorOf("publicBaseUrl")} />
        </div>
        {exampleBase ? <Snippet base={exampleBase} /> : <GatewayBasePrompt />}
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.clientSettings.forwarded")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.clientSettings.forwardedHint")}
          </p>
        </div>
        <textarea
          aria-label={t("pages.clientSettings.forwarded")}
          {...invalidProps("forwarded")}
          className="min-h-[72px] w-full rounded-md border border-[color:var(--border-default)] bg-transparent px-3 py-2 font-mono text-xs outline-none focus-visible:border-[color:var(--red-folk)]"
          placeholder={t("pages.clientSettings.forwardedPlaceholder")}
          value={form.forwarded}
          onChange={(e) => edit("forwarded", { forwarded: e.target.value })}
        />
        <FieldError id={errorId("forwarded")} error={errorOf("forwarded")} />
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          <span>{t("pages.clientSettings.alwaysPropagated")}</span>
          {dto.always_propagated.map((h) => (
            <Badge key={h} tone="info" className="font-mono">
              {h}
            </Badge>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          <span>{t("pages.clientSettings.neverForwarded")}</span>
          {dto.reserved.map((h) => (
            <Badge key={h} tone="neutral" className="font-mono">
              {h}
            </Badge>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.clientSettings.injected")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.clientSettings.injectedHint")}
          </p>
        </div>
        <div className="flex flex-col gap-2">
          {form.injected.length === 0 && (
            <p className="text-xs text-[color:var(--text-subtle)]">
              {t("pages.clientSettings.injectedNone")}
            </p>
          )}
          {form.injected.map((row, i) => (
            <div key={row.id} className="flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <Input
                  className="max-w-[220px] font-mono text-xs"
                  aria-label={t("pages.clientSettings.injectedName", { index: i + 1 })}
                  {...invalidProps(nameKey(row))}
                  placeholder="x-partner-id"
                  value={row.name}
                  onChange={(e) =>
                    edit(nameKey(row), {
                      injected: form.injected.map((r) =>
                        r.id === row.id ? { ...r, name: e.target.value } : r,
                      ),
                    })
                  }
                />
                <Input
                  className="flex-1 font-mono text-xs"
                  aria-label={t("pages.clientSettings.injectedValue", { index: i + 1 })}
                  {...invalidProps(valueKey(row))}
                  placeholder={t("pages.clientSettings.injectedValuePlaceholder")}
                  value={row.value}
                  onChange={(e) =>
                    edit(valueKey(row), {
                      injected: form.injected.map((r) =>
                        r.id === row.id ? { ...r, value: e.target.value } : r,
                      ),
                    })
                  }
                />
                <DeleteIconButton
                  label={t("pages.clientSettings.injectedRemove", { index: i + 1 })}
                  onClick={() => set({ injected: form.injected.filter((r) => r.id !== row.id) })}
                />
              </div>
              <FieldError id={errorId(nameKey(row))} error={errorOf(nameKey(row))} />
              <FieldError id={errorId(valueKey(row))} error={errorOf(valueKey(row))} />
            </div>
          ))}
        </div>
        <div>
          <Button variant="outline" onClick={() => set({ injected: [...form.injected, pair()] })}>
            <Plus className="mr-1.5 h-3.5 w-3.5" />
            {t("pages.clientSettings.addHeader")}
          </Button>
        </div>
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">{t("pages.clientSettings.correlation")}</span>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.clientSettings.correlationHint")}
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="client-request-id-header"
            className="text-xs font-medium text-[color:var(--text-secondary)]"
          >
            {t("pages.clientSettings.requestIdHeader")}
          </label>
          <Input
            className="max-w-[240px] font-mono text-xs"
            aria-label={t("pages.clientSettings.requestIdHeader")}
            {...invalidProps("requestIdHeader")}
            value={form.requestIdHeader}
            onChange={(e) => edit("requestIdHeader", { requestIdHeader: e.target.value })}
          />
          <FieldError id={errorId("requestIdHeader")} error={errorOf("requestIdHeader")} />
        </div>
      </section>

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

function Snippet({ base }: { base: string }) {
  const { t } = useTranslation();
  const code = `curl ${base}/v1/chat/completions \\\n  -H "Authorization: Bearer $ROLTER_KEY" \\\n  -H "Content-Type: application/json" \\\n  -d '{"model":"fake-llm","messages":[{"role":"user","content":"ping"}]}'`;
  // the example call through the shared code block: the same copy affordance,
  // focusable scroll region and bash palette as every other snippet (#949)
  return (
    <CodeBlock
      value={code}
      language="bash"
      label={t("pages.clientSettings.exampleRequest")}
      density="compact"
    />
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(ClientSettingsScreen, "errors.resources.clientSettings");
