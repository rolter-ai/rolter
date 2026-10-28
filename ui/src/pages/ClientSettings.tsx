import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
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
import { FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsSaveBar } from "@/components/SettingsSaveBar";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Textarea } from "@/components/ui/textarea";
import { fetchClientSettings, updateClientSettings, type ClientSettingsDto } from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { serverFieldPath } from "@/lib/field-errors";
import { gatewayBase } from "@/lib/gateway";
import { useToast } from "@/lib/toast";
import type { FieldEquality } from "@/lib/use-draft";
import { useSettingsForm } from "@/lib/use-settings-form";
import { CLIENT_SETTINGS_QUERY_KEY } from "@/lib/use-gateway-base";
import { KEY_ENV } from "@/lib/snippets";
import { useScreenReady } from "@/lib/ux-react";

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

// what a save would send: the form normalised the way the server stores it, so
// a draft that differs only in spacing or case is not an unsaved change
const toPayload = (f: FormState) => ({
  public_base_url: f.publicBaseUrl.trim() === "" ? null : f.publicBaseUrl.trim(),
  forwarded_headers: splitList(f.forwarded),
  injected_headers: Object.fromEntries(
    f.injected
      .map(({ name, value }) => [name.trim().toLowerCase(), value] as const)
      .filter(([name]) => name !== ""),
  ),
  request_id_header: f.requestIdHeader.trim().toLowerCase(),
});

// what a save would send is what "changed" means: a header typed in another
// case, a blank row or the space around a comma is not an unsaved change
const EQUALS: FieldEquality<FormState> = {
  publicBaseUrl: (a, b) => a.trim() === b.trim(),
  forwarded: (a, b) => splitList(a).join(",") === splitList(b).join(","),
  injected: (a, b) => {
    const sent = (rows: HeaderPair[]) =>
      JSON.stringify(
        toPayload({ publicBaseUrl: "", forwarded: "", requestIdHeader: "", injected: rows })
          .injected_headers,
      );
    return sent(a) === sent(b);
  },
  requestIdHeader: (a, b) => a.trim().toLowerCase() === b.trim().toLowerCase(),
};

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

// the wire names a 400 carries as `field`, mapped to the form field they belong
// to; an injected header is refused by row and placed in `onError` instead
const WIRE_FIELDS: Record<string, FieldKey> = {
  public_base_url: "publicBaseUrl",
  forwarded_headers: "forwarded",
  request_id_header: "requestIdHeader",
};

function validate(form: FormState, reserved: string[], requestIdRefused: string[]): FieldErrors {
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
  } else if (requestIdRefused.includes(requestId)) {
    errors.requestIdHeader = {
      key: "pages.clientSettings.errors.requestIdRefused",
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

  const base = React.useId();
  const idOf = (key: FieldKey) => {
    if (key === "publicBaseUrl") return "client-public-base-url";
    if (key === "requestIdHeader") return "client-request-id-header";
    return `${base}-${key}`;
  };
  const source = React.useMemo(
    () => (settings.data ? fromDto(settings.data) : undefined),
    [settings.data],
  );
  const sf = useSettingsForm<FormState, FieldKey>({
    source,
    equals: EQUALS,
    fieldErrors: (draft) => ({
      order: draft ? fieldOrder(draft) : [],
      wire: WIRE_FIELDS,
      idOf,
      local:
        draft && settings.data
          ? Object.fromEntries(
              Object.entries(
                validate(draft, settings.data.reserved, settings.data.request_id_refused ?? []),
              ).map(([key, found]) => [key, t(found.key, found.values)]),
            )
          : {},
      // an injected header is refused by row: `injected_headers.<name>`
      place: (error) => {
        const path = serverFieldPath(error);
        if (!path?.path.startsWith("injected_headers.") || !(error instanceof Error)) return null;
        const name = path.path.slice("injected_headers.".length);
        const row = draft?.injected.find((r) => r.name.trim().toLowerCase() === name);
        return row ? { field: nameKey(row), message: error.message } : null;
      },
    }),
  });
  const { draft: form, fields } = sf;

  const save = useMutation({
    mutationFn: (f: FormState) => updateClientSettings(toPayload(f)),
    onSuccess: (dto) => {
      queryClient.setQueryData(CLIENT_SETTINGS_QUERY_KEY, dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: CLIENT_SETTINGS_QUERY_KEY });
      sf.commit(fromDto(dto));
      // the screen changes gateway behaviour live, so the toast says when it
      // takes effect rather than leaving a restart to be guessed at (#2076)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("pages.clientSettings.savedDetail"),
      });
    },
    onError: (error) => {
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.clientSettings") }),
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
          resource={t("errors.resources.clientSettings")}
          onRetry={() => void settings.refetch()}
          target="client-settings"
        />
      </SettingsBody>
    );
  }
  if (!form || !settings.data) return null;

  const dto = settings.data;
  const { set } = sf;
  // an edit answers the server's complaint about that field
  const edit = (key: FieldKey, patch: Partial<FormState>) => {
    set(patch);
    fields.edit(key);
  };
  // what a client would actually type, following the field as it is edited.
  // an empty field has no example: the /gw proxy needs a dashboard session, so
  // it is no address for a client (#2486)
  const exampleBase = gatewayBase(form.publicBaseUrl)?.url ?? null;

  return (
    <SettingsBody onSubmit={() => fields.submit(() => save.mutate(form))}>
      <SettingsPanel
        title={t("pages.clientSettings.baseUrl")}
        description={t("pages.clientSettings.baseUrlHint")}
      >
        <div className="flex w-full flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="client-public-base-url"
              className="text-xs font-medium text-[color:var(--text-secondary)]"
            >
              {t("pages.clientSettings.publicBaseUrl")}
            </label>
            <Input
              className="sm:min-w-[320px] font-mono text-xs"
              {...fields.inputProps("publicBaseUrl")}
              placeholder={BASE_URL_PLACEHOLDER}
              value={form.publicBaseUrl}
              onChange={(e) => edit("publicBaseUrl", { publicBaseUrl: e.target.value })}
            />
            <FieldError
              id={fields.errorId("publicBaseUrl")}
              error={fields.errorOf("publicBaseUrl")}
            />
          </div>
          {exampleBase ? <Snippet base={exampleBase} /> : <GatewayBasePrompt />}
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.clientSettings.forwarded")}
        description={t("pages.clientSettings.forwardedHint")}
      >
        <div className="flex w-full flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <Textarea
              aria-label={t("pages.clientSettings.forwarded")}
              {...fields.inputProps("forwarded")}
              className="min-h-[72px] font-mono text-xs"
              placeholder={t("pages.clientSettings.forwardedPlaceholder")}
              value={form.forwarded}
              onChange={(e) => edit("forwarded", { forwarded: e.target.value })}
            />
            <FieldError id={fields.errorId("forwarded")} error={fields.errorOf("forwarded")} />
          </div>
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
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.clientSettings.injected")}
        description={t("pages.clientSettings.injectedHint")}
      >
        <div className="flex w-full flex-col gap-3.5">
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
                    {...fields.inputProps(nameKey(row))}
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
                    {...fields.inputProps(valueKey(row))}
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
                <FieldError
                  id={fields.errorId(nameKey(row))}
                  error={fields.errorOf(nameKey(row))}
                />
                <FieldError
                  id={fields.errorId(valueKey(row))}
                  error={fields.errorOf(valueKey(row))}
                />
              </div>
            ))}
          </div>
          <div>
            <Button variant="outline" onClick={() => set({ injected: [...form.injected, pair()] })}>
              <Plus className="mr-1.5 h-3.5 w-3.5" />
              {t("pages.clientSettings.addHeader")}
            </Button>
          </div>
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.clientSettings.correlation")}
        description={t("pages.clientSettings.correlationHint")}
      >
        <div className="flex w-full flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="client-request-id-header"
              className="text-xs font-medium text-[color:var(--text-secondary)]"
            >
              {t("pages.clientSettings.requestIdHeader")}
            </label>
            <Input
              className="max-w-[240px] font-mono text-xs"
              {...fields.inputProps("requestIdHeader")}
              value={form.requestIdHeader}
              onChange={(e) => edit("requestIdHeader", { requestIdHeader: e.target.value })}
            />
            <FieldError
              id={fields.errorId("requestIdHeader")}
              error={fields.errorOf("requestIdHeader")}
            />
          </div>
        </div>
      </SettingsPanel>

      <SettingsSaveBar form={sf} pending={save.isPending} updatedAt={dto.updated_at} />
    </SettingsBody>
  );
}

function Snippet({ base }: { base: string }) {
  const { t } = useTranslation();
  const code = `curl ${base}/v1/chat/completions \\\n  -H "Authorization: Bearer $${KEY_ENV}" \\\n  -H "Content-Type: application/json" \\\n  -d '{"model":"fake-llm","messages":[{"role":"user","content":"ping"}]}'`;
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
export default superadminOnly(
  ClientSettingsScreen,
  "errors.resources.clientSettings",
  "client_settings:read",
);
