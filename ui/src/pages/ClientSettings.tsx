import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { Input } from "@/components/ui/input";
import {
  fetchClientSettings,
  updateClientSettings,
  type ClientSettingsDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
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

function validate(form: FormState, reserved: string[]): string | null {
  const url = form.publicBaseUrl.trim();
  if (url !== "" && !/^https?:\/\//.test(url)) {
    return "Base URL must start with http:// or https://.";
  }
  const forwarded = splitList(form.forwarded);
  for (const name of forwarded) {
    if (!HEADER_NAME.test(name)) return `'${name}' is not a valid header name.`;
    if (reserved.includes(name)) return `'${name}' is managed by the gateway.`;
  }
  if (forwarded.length > 64) return "At most 64 forwarded headers are allowed.";

  const seen = new Set<string>();
  for (const { name, value } of form.injected) {
    const lower = name.trim().toLowerCase();
    if (lower === "" && value.trim() === "") continue;
    if (!HEADER_NAME.test(lower)) return `'${name}' is not a valid header name.`;
    if (reserved.includes(lower)) return `'${lower}' is managed by the gateway.`;
    if (seen.has(lower)) return `'${lower}' is listed twice.`;
    seen.add(lower);
    if (/[\u0000-\u001f\u007f]/.test(value)) {
      return `Value for '${lower}' must not contain control characters.`;
    }
  }
  if (seen.size > 64) return "At most 64 injected headers are allowed.";

  const requestId = form.requestIdHeader.trim().toLowerCase();
  if (!HEADER_NAME.test(requestId)) return "Request ID header is not a valid header name.";
  if (reserved.includes(requestId)) return `'${requestId}' is managed by the gateway.`;
  return null;
}

// deployment-wide client settings, persisted via /api/v1/client-settings
// (superadmin only): the base URL the dashboard hands out, and what the gateway
// does with headers on the upstream leg
function ClientSettingsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const settings = useQuery({
    queryKey: ["client-settings"],
    queryFn: fetchClientSettings,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `settings` is the query the user is actually waiting on for this screen
  useScreenReady(!settings.isLoading);
  useErrorState(!!settings.error, "client-settings");

  const [form, setForm] = React.useState<FormState | null>(null);
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
      queryClient.setQueryData(["client-settings"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["client-settings"] });
      setForm(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.clientSettings") }),
      });
    },
    onError: (error) => {
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
  const localError = validate(form, dto.reserved);
  // what a client would actually type; falls back to this dashboard's origin
  const effectiveBase =
    form.publicBaseUrl.trim() ||
    (typeof window === "undefined" ? "https://your-gateway.example.com" : window.location.origin);

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">Base URL</span>
          <p className="mt-1 text-sm text-muted-foreground">
            The address clients point their SDK at. Set this when the gateway sits
            behind a load balancer or a custom domain, so the snippet below matches
            what a caller can actually reach. Leave it empty to use this
            dashboard&rsquo;s own origin.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="client-public-base-url" className="text-xs font-medium text-[color:var(--text-secondary)]">
            Public base URL
          </label>
          <Input
            id="client-public-base-url"
            className="min-w-[320px] font-mono text-xs"
            aria-label="Public base URL"
            placeholder={effectiveBase}
            value={form.publicBaseUrl}
            onChange={(e) => set({ publicBaseUrl: e.target.value })}
          />
        </div>
        <Snippet base={effectiveBase} />
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">Forwarded request headers</span>
          <p className="mt-1 text-sm text-muted-foreground">
            Inbound client headers passed through to the upstream provider,
            comma or newline separated. Everything not listed is dropped at the
            gateway.
          </p>
        </div>
        <textarea
          aria-label="Forwarded request headers"
          className="min-h-[72px] w-full rounded-md border border-[color:var(--border-default)] bg-transparent px-3 py-2 font-mono text-xs outline-none focus-visible:border-[color:var(--red-folk)]"
          placeholder="x-tenant-id, x-session-id"
          value={form.forwarded}
          onChange={(e) => set({ forwarded: e.target.value })}
        />
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          <span>Always propagated:</span>
          {dto.always_propagated.map((h) => (
            <Badge key={h} tone="info" className="font-mono">
              {h}
            </Badge>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          <span>Never forwarded:</span>
          {dto.reserved.map((h) => (
            <Badge key={h} tone="neutral" className="font-mono">
              {h}
            </Badge>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">Injected upstream headers</span>
          <p className="mt-1 text-sm text-muted-foreground">
            Static headers the gateway adds to every upstream request. A
            trace-context header of the same name still wins, so correlation
            cannot be broken from here. Values are treated as deployment secrets
            and are never written to the audit log.
          </p>
        </div>
        <div className="flex flex-col gap-2">
          {form.injected.length === 0 && (
            <p className="text-xs text-[color:var(--text-subtle)]">
              No headers injected — upstream requests carry only what the gateway
              and the caller already send.
            </p>
          )}
          {form.injected.map((row, i) => (
            <div key={row.id} className="flex items-center gap-2">
              <Input
                className="max-w-[220px] font-mono text-xs"
                aria-label={`Injected header name ${i + 1}`}
                placeholder="x-partner-id"
                value={row.name}
                onChange={(e) =>
                  set({
                    injected: form.injected.map((r) =>
                      r.id === row.id ? { ...r, name: e.target.value } : r,
                    ),
                  })
                }
              />
              <Input
                className="flex-1 font-mono text-xs"
                aria-label={`Injected header value ${i + 1}`}
                placeholder="value"
                value={row.value}
                onChange={(e) =>
                  set({
                    injected: form.injected.map((r) =>
                      r.id === row.id ? { ...r, value: e.target.value } : r,
                    ),
                  })
                }
              />
              <button
                type="button"
                aria-label={`Remove injected header ${i + 1}`}
                className="flex h-8 w-8 flex-none items-center justify-center rounded-md text-[color:var(--text-subtle)] transition-colors hover:text-[color:var(--status-danger-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                onClick={() =>
                  set({ injected: form.injected.filter((r) => r.id !== row.id) })
                }
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          ))}
        </div>
        <div>
          <Button
            variant="outline"
            onClick={() => set({ injected: [...form.injected, pair()] })}
          >
            <Plus className="mr-1.5 h-3.5 w-3.5" />
            Add header
          </Button>
        </div>
      </section>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">Correlation</span>
          <p className="mt-1 text-sm text-muted-foreground">
            Header carrying the end-to-end request id. The gateway reuses the
            caller&rsquo;s value when present, mints one when absent, and echoes it
            on the response so a client can find its own call in the logs.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="client-request-id-header" className="text-xs font-medium text-[color:var(--text-secondary)]">
            Request ID header
          </label>
          <Input
            id="client-request-id-header"
            className="max-w-[240px] font-mono text-xs"
            aria-label="Request ID header"
            value={form.requestIdHeader}
            onChange={(e) => set({ requestIdHeader: e.target.value })}
          />
        </div>
      </section>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {localError && <span className="text-xs text-[color:var(--status-danger-text)]">{localError}</span>}
        <Button disabled={save.isPending || localError !== null} onClick={() => save.mutate(form)}>
          {save.isPending ? "Saving…" : "Save Changes"}
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
