import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  Lock,
  LockOpen,
  Plug,
  Plus,
  Trash2,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { FormSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { InfoHint } from "@/components/ui/info-hint";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  createRoute,
  createRouteTarget,
  deleteRouteTarget,
  fetchCurrencySettings,
  fetchModelPrices,
  fetchRouteTargets,
  fetchTeams,
  fetchUsers,
  fetchVirtualKeys,
  isConvertible,
  ROLES,
  setRouteAdvanced,
  setRouteEnabled,
  STRATEGIES,
  updateRouteParams,
  upsertModelPrice,
  type EffectiveModelDto,
  type ProviderRow,
  type RouteRow,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useFormTelemetry } from "@/lib/ux-react";

// ---------------------------------------------------------------------------
// draft model — one object carries the whole form (see design handoff)
// ---------------------------------------------------------------------------

export type ModelSheetMode = "add" | "edit" | "view";

type Modality = "chat" | "embedding" | "image" | "audio";
type LockMode = "lockAll" | "unlockAll" | "manual";
type ParamType = "string" | "int" | "float" | "boolean" | "enum";

interface DraftParam {
  key: string;
  value: string;
  type: ParamType;
  locked: boolean;
  custom: boolean;
  opts?: string[] | null;
}

interface DraftHeader {
  key: string;
  value: string;
  locked: boolean;
}

interface Caps {
  streaming: boolean;
  tools: boolean;
  vision: boolean;
  json: boolean;
  reasoning: boolean;
}

interface ModelDraft {
  providerId: string;
  modality: Modality;
  upstreamName: string;
  alias: string;
  baseUrl: string;
  description: string;
  enabled: boolean;
  paramMode: LockMode;
  params: DraftParam[];
  caps: Caps;
  price: {
    input: string;
    output: string;
    cacheWrite: string;
    cacheRead: string;
    perRequest: string;
    currency: string;
  };
  net: {
    insecureTls: boolean;
    allowAdditional: boolean;
    rpm: string;
    tpm: string;
    concurrency: string;
    timeoutMs: string;
    retries: string;
    weight: string;
    context: string;
    maxOutput: string;
  };
  headerMode: LockMode;
  headers: DraftHeader[];
  rbac: {
    minRole: string;
    visibility: "public" | "restricted";
    teams: string[];
    vkeys: string[];
    users: string[];
  };
}

/**
 * Where the prices in this sheet come from.
 *
 * They come from whoever types them: rolter ships no pricing catalog, so the
 * link points at our own docs for how a request's cost is computed. It used to
 * point at a competing gateway's datasheet, presented as the source of numbers
 * that were never theirs (#977).
 */
const PRICING_DOCS_URL =
  "https://github.com/rolter-ai/rolter/blob/master/user-docs/observability/logs-and-cost.mdx#cost-tracking";

const MODALITIES: Modality[] = ["chat", "embedding", "image", "audio"];
const PARAM_TYPES: ParamType[] = ["string", "int", "float", "boolean", "enum"];

function paramDefs(modality: Modality, reasoning: boolean): DraftParam[] {
  const p = (key: string, type: ParamType, opts?: string[]): DraftParam => ({
    key,
    value: "",
    locked: false,
    type,
    custom: false,
    opts: opts ?? null,
  });
  if (modality === "embedding") {
    return [p("dimensions", "int"), p("encoding_format", "enum", ["", "float", "base64"])];
  }
  if (modality === "image") {
    return [
      p("size", "enum", ["", "256x256", "512x512", "1024x1024", "1792x1024", "1024x1792"]),
      p("quality", "enum", ["", "standard", "hd"]),
      p("style", "enum", ["", "vivid", "natural"]),
      p("n", "int"),
    ];
  }
  if (modality === "audio") {
    return [
      p("voice", "string"),
      p("speed", "float"),
      p("response_format", "enum", ["", "mp3", "opus", "aac", "flac", "wav"]),
      p("language", "string"),
    ];
  }
  const base = [
    p("temperature", "float"),
    p("top_p", "float"),
    p("top_k", "int"),
    p("max_tokens", "int"),
    p("frequency_penalty", "float"),
    p("presence_penalty", "float"),
    p("stop", "string"),
    p("seed", "int"),
  ];
  if (reasoning) base.push(p("reasoning_effort", "enum", ["", "low", "medium", "high"]));
  return base;
}

function defaultCaps(modality: Modality): Caps {
  if (modality === "chat") {
    return { streaming: true, tools: true, vision: false, json: true, reasoning: false };
  }
  if (modality === "audio") {
    return { streaming: true, tools: false, vision: false, json: false, reasoning: false };
  }
  return { streaming: false, tools: false, vision: false, json: false, reasoning: false };
}

function blankDraft(providerId: string): ModelDraft {
  return {
    providerId,
    modality: "chat",
    upstreamName: "",
    alias: "",
    baseUrl: "",
    description: "",
    enabled: true,
    paramMode: "manual",
    params: paramDefs("chat", false),
    caps: defaultCaps("chat"),
    price: { input: "", output: "", cacheWrite: "", cacheRead: "", perRequest: "", currency: "USD" },
    net: {
      insecureTls: false,
      allowAdditional: false,
      rpm: "",
      tpm: "",
      concurrency: "",
      timeoutMs: "",
      retries: "",
      weight: "100",
      context: "",
      maxOutput: "",
    },
    headerMode: "manual",
    headers: [],
    rbac: { minRole: "member", visibility: "public", teams: [], vkeys: [], users: [] },
  };
}

/**
 * Seed the draft from a route's stored `advanced` blob (#1178).
 *
 * `RouteRow.advanced` is an `AdvancedModelConfig` (rolter-core), written by
 * `setRouteAdvanced` and read back by nothing until now: the catalog metadata,
 * limits, headers and visibility a route already carried opened as blank
 * fields, so re-saving quietly proposed clearing them.
 *
 * Every field is optional on the backend, so each one is read defensively —
 * an object shaped by an older or newer release must still seed what it can.
 */
function seedAdvanced(draft: ModelDraft, advanced: Record<string, unknown>) {
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const num = (v: unknown) => (typeof v === "number" ? String(v) : "");
  const strings = (v: unknown) =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  const obj = (v: unknown): Record<string, unknown> =>
    v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};

  if (MODALITIES.includes(advanced.model_type as Modality)) {
    draft.modality = advanced.model_type as Modality;
    draft.caps = defaultCaps(draft.modality);
  }
  const capabilities = strings(advanced.capabilities);
  if (capabilities.length) {
    for (const key of Object.keys(draft.caps) as (keyof Caps)[]) {
      draft.caps[key] = capabilities.includes(key);
    }
  }
  draft.params = paramDefs(draft.modality, draft.caps.reasoning);
  draft.baseUrl = str(advanced.base_url);
  draft.description = str(advanced.description);

  const pricing = obj(advanced.pricing);
  draft.price.cacheWrite = num(pricing.cache_write_per_mtok);
  draft.price.perRequest = num(pricing.image_per_unit);

  const limits = obj(advanced.limits);
  draft.net.rpm = num(limits.rpm);
  draft.net.tpm = num(limits.tpm);
  draft.net.concurrency = num(limits.concurrency);
  // the backend stores whole seconds; the field is milliseconds
  draft.net.timeoutMs =
    typeof limits.timeout_secs === "number" ? String(limits.timeout_secs * 1000) : "";
  draft.net.retries = num(limits.retries);
  draft.net.context = num(limits.context_window);
  draft.net.maxOutput = num(limits.output_tokens);
  draft.net.insecureTls = advanced.insecure_tls === true;
  draft.net.allowAdditional = Object.keys(obj(advanced.additional_fields)).length > 0;

  const locked = new Set(strings(advanced.locked_headers));
  draft.headers = Object.entries(obj(advanced.headers)).map(([key, value]) => ({
    key,
    value: typeof value === "string" ? value : String(value),
    locked: locked.has(key),
  }));
  // "every header locked" and "none locked" are the two the sheet can round-trip
  // exactly; anything in between is the manual mode it already has for that
  draft.headerMode = draft.headers.length === 0
    ? "manual"
    : draft.headers.every((h) => h.locked)
      ? "lockAll"
      : draft.headers.some((h) => h.locked)
        ? "manual"
        : "unlockAll";

  const visibility = obj(advanced.visibility);
  draft.rbac.minRole = str(visibility.minimum_role) || draft.rbac.minRole;
  draft.rbac.teams = strings(visibility.allowed_team_ids);
  draft.rbac.vkeys = strings(visibility.allowed_key_ids);
  draft.rbac.users = strings(visibility.allowed_user_ids);
  draft.rbac.visibility =
    draft.rbac.teams.length + draft.rbac.vkeys.length + draft.rbac.users.length > 0
      ? "restricted"
      : "public";
}

/**
 * Serialize the draft back into an `AdvancedModelConfig` — the inverse of
 * `seedAdvanced()` (#1189).
 *
 * `stored` is the blob the route already carries and is the base of the
 * result, so a field this form does not model — the per-route guardrail
 * selection, anything a newer control plane added — survives a save instead of
 * being reset to its serde default.
 */
function advancedToApi(
  draft: ModelDraft,
  stored: Record<string, unknown>,
): Record<string, unknown> {
  const obj = (v: unknown): Record<string, unknown> =>
    v && typeof v === "object" && !Array.isArray(v)
      ? { ...(v as Record<string, unknown>) }
      : {};
  // a limit of 0 is refused by `validate_advanced`; blank and 0 both read as
  // "inherit the gateway/provider setting", so neither is sent
  const limit = (v: string) => {
    const n = Math.trunc(Number(v));
    return v.trim() !== "" && Number.isFinite(n) && n > 0 ? n : undefined;
  };
  const price = (v: string) => {
    const n = Number(v);
    return v.trim() !== "" && Number.isFinite(n) ? n : undefined;
  };
  // an absent field is absent, not null: every one is `Option`/`default` on the
  // backend and a null would fail to deserialize
  const put = (target: Record<string, unknown>, key: string, value: unknown) => {
    if (value === undefined) delete target[key];
    else target[key] = value;
  };

  const out = { ...stored };
  out.model_type = draft.modality;
  out.capabilities = Object.entries(draft.caps)
    .filter(([, on]) => on)
    .map(([key]) => key);
  put(out, "base_url", draft.baseUrl.trim() || undefined);
  put(out, "description", draft.description.trim() || undefined);

  // the audio rates have no field on this sheet, so they are carried through
  // rather than dropped by a save that never showed them
  const pricing = obj(stored.pricing);
  put(pricing, "cache_write_per_mtok", price(draft.price.cacheWrite));
  put(pricing, "image_per_unit", price(draft.price.perRequest));
  put(out, "pricing", Object.keys(pricing).length > 0 ? pricing : undefined);

  const limits: Record<string, unknown> = {};
  put(limits, "rpm", limit(draft.net.rpm));
  put(limits, "tpm", limit(draft.net.tpm));
  put(limits, "concurrency", limit(draft.net.concurrency));
  put(limits, "retries", limit(draft.net.retries));
  put(limits, "context_window", limit(draft.net.context));
  put(limits, "output_tokens", limit(draft.net.maxOutput));
  // the field is milliseconds and the backend stores whole seconds; a
  // sub-second timeout rounds up to 1 rather than to the 0 it would refuse
  const timeoutMs = Number(draft.net.timeoutMs);
  if (draft.net.timeoutMs.trim() !== "" && Number.isFinite(timeoutMs) && timeoutMs > 0) {
    limits.timeout_secs = Math.max(1, Math.round(timeoutMs / 1000));
  }
  out.limits = limits;

  out.insecure_tls = draft.net.insecureTls;
  // the switch has no field of its own on the wire — `seedAdvanced` reads it
  // off whether the stored map has anything in it — so the map is carried
  // through untouched rather than invented here (#1271)
  out.additional_fields = draft.net.allowAdditional ? obj(stored.additional_fields) : {};

  const headers: Record<string, string> = {};
  const lockedHeaders: string[] = [];
  for (const h of draft.headers) {
    const key = h.key.trim();
    if (!key) continue;
    headers[key] = h.value;
    if (effLock(draft.headerMode, h.locked)) lockedHeaders.push(key);
  }
  out.headers = headers;
  out.locked_headers = lockedHeaders;

  // the allow-lists are ids, and the control plane parses each one as a uuid;
  // a public model carries none of them
  const restricted = draft.rbac.visibility === "restricted";
  out.visibility = {
    minimum_role: draft.rbac.minRole,
    allowed_team_ids: restricted ? draft.rbac.teams : [],
    allowed_key_ids: restricted ? draft.rbac.vkeys : [],
    allowed_user_ids: restricted ? draft.rbac.users : [],
  };
  return out;
}

// seed draft params/lock-mode from a stored route's params + override policy
// (the same shapes ParamsEditor reads/writes: allow/deny base + deny list)
function seedParams(
  draft: ModelDraft,
  params: Record<string, unknown>,
  policy: Record<string, unknown>,
) {
  const deny = Array.isArray(policy.deny)
    ? policy.deny.filter((x): x is string => typeof x === "string")
    : [];
  draft.paramMode =
    policy.mode === "deny" ? "lockAll" : deny.length > 0 ? "manual" : "unlockAll";
  const lockedKeys = new Set(deny);
  const byKey = new Map(draft.params.map((p) => [p.key, p]));
  for (const [key, value] of Object.entries(params)) {
    const row = byKey.get(key);
    const text = typeof value === "string" ? value : JSON.stringify(value);
    if (row) {
      row.value = text;
      row.locked = lockedKeys.has(key);
    } else {
      const type: ParamType =
        typeof value === "number"
          ? Number.isInteger(value)
            ? "int"
            : "float"
          : typeof value === "boolean"
            ? "boolean"
            : "string";
      draft.params.push({
        key,
        value: text,
        type,
        locked: lockedKeys.has(key),
        custom: true,
      });
    }
  }
}

function effLock(mode: LockMode, locked: boolean): boolean {
  return mode === "lockAll" ? true : mode === "unlockAll" ? false : locked;
}

function coerce(value: string, type: ParamType): unknown {
  if (type === "int" || type === "float") {
    const n = Number(value);
    return Number.isNaN(n) ? value : n;
  }
  if (type === "boolean") return value === "true";
  return value;
}

// serialize the draft params into the control-api params + override-policy
// shapes (allow-base with a deny list of locked keys; lockAll = deny-base)
function paramsToApi(draft: ModelDraft): {
  params: Record<string, unknown>;
  paramPolicy: Record<string, unknown>;
} {
  const params: Record<string, unknown> = {};
  const denied: string[] = [];
  for (const p of draft.params) {
    const key = p.key.trim();
    if (!key || p.value.trim() === "") continue;
    params[key] = coerce(p.value, p.type);
    if (draft.paramMode === "manual" && p.locked) denied.push(key);
  }
  const paramPolicy =
    draft.paramMode === "lockAll"
      ? { mode: "deny", allow: [], deny: [] }
      : { mode: "allow", allow: [], deny: denied };
  return { params, paramPolicy };
}

/**
 * Live JSON of what saving this draft sends, request by request.
 *
 * It used to serialize a shape of its own — `network.custom_headers`,
 * `access.min_role` — that no endpoint took, so the pane read as a
 * confirmation of fields the sheet then dropped (#1189). Every key below is a
 * body the save actually puts on the wire.
 */
function buildPreview(
  draft: ModelDraft,
  providerName: string,
  advanced: Record<string, unknown>,
) {
  const { params, paramPolicy } = paramsToApi(draft);
  const upstream = draft.upstreamName.trim();
  const publicName = draft.alias.trim() || upstream;
  const hasPricing = draft.price.input.trim() !== "" || draft.price.output.trim() !== "";
  const obj = {
    route: { model: publicName || undefined, enabled: draft.enabled },
    target: draft.providerId
      ? {
          provider: providerName || undefined,
          upstream_model: upstream !== publicName ? upstream : undefined,
          weight: Number(draft.net.weight) || 1,
        }
      : undefined,
    params,
    param_policy: paramPolicy,
    model_price: hasPricing
      ? {
          model: publicName || undefined,
          input_per_mtok: draft.price.input.trim() || "0",
          output_per_mtok: draft.price.output.trim() || "0",
          cached_input_per_mtok: draft.price.cacheRead.trim() || undefined,
          currency: draft.price.currency,
        }
      : undefined,
    advanced,
  };
  return JSON.stringify(obj, null, 2);
}

// ---------------------------------------------------------------------------
// small presentational pieces
// ---------------------------------------------------------------------------

function FieldLabel({
  label,
  required,
  info,
  htmlFor,
  id,
}: {
  label: string;
  required?: boolean;
  info?: string;
  /** the control this names, so no label dangles */
  htmlFor?: string;
  /** for a group (a segmented control) that is `aria-labelledby` this id */
  id?: string;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <label
        id={id}
        htmlFor={htmlFor}
        className="text-xs font-medium text-[color:var(--text-secondary)]"
      >
        {label}
      </label>
      {required && <span className="text-xs text-[color:var(--status-danger-text)]">*</span>}
      {info && <InfoHint text={info} label={`About ${label}`} />}
    </div>
  );
}

function FieldError({ error }: { error?: string }) {
  if (!error) return null;
  return <p className="text-xs leading-snug text-[color:var(--status-danger-text)]">{error}</p>;
}

function Section({
  title,
  info,
  open,
  onToggle,
  children,
  className,
}: {
  title: string;
  info?: string;
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className="rounded-[10px] border border-[color:var(--border-subtle)]">
      {/* the toggle is a real button and the InfoHint sits beside it rather
          than inside it: this used to be a `div role="button"` wrapping the
          hint's own button, which is a nested interactive control — invalid
          HTML that a screen reader announces as one confused thing, and an
          axe `nested-interactive` failure (#1201) */}
      <div className="flex w-full items-center gap-2.5 px-[15px]">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={open}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 py-[13px] text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <span className="text-sm font-semibold">{title}</span>
          <ChevronDown
            className={cn(
              "ml-auto h-4 w-4 text-[color:var(--text-subtle)] transition-transform duration-[120ms]",
              open && "rotate-180",
            )}
          />
        </button>
        {info && <InfoHint text={info} label={`About ${title}`} />}
      </div>
      {open && (
        <div
          className={cn(
            "border-t border-[color:var(--border-subtle)] px-[15px] pb-4 pt-3.5",
            className,
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}

function Segmented<T extends string>({
  value,
  options,
  onChange,
  disabled,
  labelledBy,
  ariaLabel,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
  /** id of the FieldLabel naming this group */
  labelledBy?: string;
  /** a name for a group with no visible label */
  ariaLabel?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-label={ariaLabel}
      className="inline-flex w-fit rounded-md bg-[color:var(--surface-subtle)] p-0.5"
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          disabled={disabled}
          onClick={() => onChange(o.value)}
          className={cn(
            "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
            "rounded px-2.5 py-1 text-xs font-medium transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
            value === o.value
              ? "bg-[color:var(--surface-base)] text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function LockButton({
  locked,
  onToggle,
  disabled,
}: {
  locked: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
  const label = locked
    ? "Locked — clients can't override. Click to unlock."
    : "Unlocked — clients can override. Click to lock.";

  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onToggle}
      aria-pressed={locked}
      title={label}
      aria-label={label}
      className={cn(
        "flex h-8 w-8 flex-none items-center justify-center rounded-md border transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        locked
          ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-[color:var(--red-folk-text)]"
          : "border-[color:var(--border-subtle)] bg-transparent text-[color:var(--text-subtle)]",
      )}
    >
      {locked ? <Lock className="h-3.5 w-3.5" /> : <LockOpen className="h-3.5 w-3.5" />}
    </button>
  );
}

function ChipGroup({
  label,
  options,
  selected,
  onToggle,
  disabled,
}: {
  label: string;
  /**
   * the rows to pick from. `id` is what the draft stores and what
   * `visibility.allowed_*_ids` carries — the control plane parses each one as
   * a uuid — while `name` is what the operator reads (#1189)
   */
  options: { id: string; name: string }[];
  selected: string[];
  onToggle: (v: string) => void;
  disabled?: boolean;
}) {
  const id = React.useId();
  return (
    <div className="space-y-1.5" role="group" aria-labelledby={id}>
      <span id={id} className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </span>
      <div className="flex flex-wrap gap-1.5">
        {options.length === 0 && (
          <p className="text-xs text-muted-foreground">none available</p>
        )}
        {options.map((o) => {
          const on = selected.includes(o.id);
          return (
            <button
              key={o.id}
              type="button"
              disabled={disabled}
              onClick={() => onToggle(o.id)}
              aria-pressed={on}
              className={cn(
                "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                "inline-flex h-7 items-center rounded-full border px-2.5 font-mono text-xs transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
                on
                  ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-foreground"
                  : "border-[color:var(--border-subtle)] bg-transparent text-muted-foreground",
              )}
            >
              {o.name}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function SwitchRow({
  title,
  hint,
  info,
  checked,
  onChange,
  disabled,
}: {
  title: string;
  hint?: string;
  info?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center gap-3 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-sm">{title}</span>
          {info && <InfoHint text={info} label={`About ${title}`} />}
        </div>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} disabled={disabled} aria-label={title} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// the sheet
// ---------------------------------------------------------------------------

const SECTIONS = [
  "general",
  "params",
  "caps",
  "pricing",
  "advanced",
  "headers",
  "rbac",
  "preview",
] as const;
type SectionKey = (typeof SECTIONS)[number];

export interface ModelSheetProps {
  open: boolean;
  mode: ModelSheetMode;
  onOpenChange: (open: boolean) => void;
  projectId: string | null;
  orgId: string | null;
  providers: ProviderRow[];
  // edit mode: the db route being edited
  route?: RouteRow | null;
  // view mode: the readonly config-owned model
  configModel?: EffectiveModelDto | null;
  // every effective model, for name-conflict checks + duplicate-from
  models: EffectiveModelDto[];
  routes: RouteRow[];
  onDone: () => void;
}

export function ModelSheet({
  open,
  mode,
  onOpenChange,
  projectId,
  orgId,
  providers,
  route,
  configModel,
  models,
  routes,
  onDone,
}: ModelSheetProps) {
  const queryClient = useQueryClient();
  const readonly = mode === "view";

  const [draft, setDraft] = React.useState<ModelDraft>(() => blankDraft(""));
  const [secOpen, setSecOpen] = React.useState<Record<SectionKey, boolean>>({
    general: true,
    params: false,
    caps: false,
    pricing: false,
    advanced: false,
    headers: false,
    rbac: false,
    preview: false,
  });
  const [dupFrom, setDupFrom] = React.useState("");
  const [testState, setTestState] = React.useState<"idle" | "testing" | "ok">("idle");
  const initialRef = React.useRef("");
  // the advanced payload as it was seeded, so a save can skip the extra PUT
  // when the operator changed nothing on that half of the form
  const initialAdvancedRef = React.useRef("");

  // data for edit-mode prefill
  const targets = useQuery({
    queryKey: ["route-targets", route?.id],
    queryFn: () => fetchRouteTargets(route!.id),
    enabled: open && mode === "edit" && !!route,
  });
  const prices = useQuery({
    queryKey: ["model-prices"],
    queryFn: fetchModelPrices,
    enabled: open && mode === "edit",
  });
  // the currency chooser is the deployment's rate table, not a literal (#965).
  // deployment config, so it never goes stale within a session
  const currency = useQuery({
    queryKey: ["currency-settings"],
    queryFn: fetchCurrencySettings,
    enabled: open,
    staleTime: Infinity,
    retry: false,
  });

  // rbac chip sources (best-effort; sections stay usable without them)
  const teams = useQuery({
    queryKey: ["teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: open && !!orgId,
    retry: false,
  });
  const vkeys = useQuery({
    queryKey: ["virtual-keys", projectId],
    queryFn: () => fetchVirtualKeys(projectId as string),
    enabled: open && !!projectId,
    retry: false,
  });
  const users = useQuery({
    queryKey: ["users", orgId],
    queryFn: () => fetchUsers(orgId as string),
    enabled: open && !!orgId,
    retry: false,
  });

  const editLoading = mode === "edit" && (targets.isLoading || prices.isLoading);

  // seed the draft once per open (edit mode waits for targets + prices)
  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current || editLoading) return;
    seededRef.current = true;
    const d = blankDraft(providers[0]?.id ?? "");
    if (mode === "edit" && route) {
      const target = targets.data?.[0];
      d.providerId = target?.provider_id ?? "";
      d.upstreamName = target?.upstream_model || route.model;
      d.alias = target?.upstream_model ? route.model : "";
      d.enabled = route.enabled;
      seedAdvanced(d, route.advanced ?? {});
      d.net.weight = target ? String(target.weight) : "100";
      seedParams(d, route.params ?? {}, route.param_policy ?? {});
      const price = prices.data?.find((p) => p.model === route.model);
      if (price) {
        d.price.input = price.input_per_mtok;
        d.price.output = price.output_per_mtok;
        d.price.cacheRead = price.cached_input_per_mtok ?? "";
        d.price.currency = price.currency || "USD";
      }
    } else if (mode === "view" && configModel) {
      d.providerId = "";
      d.upstreamName = configModel.model;
    }
    setDraft(d);
    setDupFrom("");
    setTestState("idle");
    setSecOpen({
      general: true,
      params: false,
      caps: false,
      pricing: false,
      advanced: false,
      headers: false,
      rbac: false,
      preview: false,
    });
    initialRef.current = JSON.stringify(d);
    initialAdvancedRef.current = JSON.stringify(
      advancedToApi(d, mode === "edit" ? (route?.advanced ?? {}) : {}),
    );
  }, [open, mode, route, configModel, providers, targets.data, prices.data, editLoading]);

  const dirty = !readonly && initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const { t } = useTranslation();
  const toast = useToast();
  const guard = React.useCallback(() => {
    if (!dirty) return true;
    return window.confirm(t("common.discardChanges"));
  }, [dirty, t]);

  const set = (patch: Partial<ModelDraft>) => setDraft((d) => ({ ...d, ...patch }));
  const setDeep = <K extends "price" | "net" | "rbac" | "caps">(
    key: K,
    patch: Partial<ModelDraft[K]>,
  ) => setDraft((d) => ({ ...d, [key]: { ...d[key], ...patch } }));
  const setParamAt = (i: number, patch: Partial<DraftParam>) =>
    setDraft((d) => ({
      ...d,
      params: d.params.map((p, idx) => (idx === i ? { ...p, ...patch } : p)),
    }));
  const setHeaderAt = (i: number, patch: Partial<DraftHeader>) =>
    setDraft((d) => ({
      ...d,
      headers: d.headers.map((h, idx) => (idx === i ? { ...h, ...patch } : h)),
    }));
  const toggleSec = (k: SectionKey) => setSecOpen((s) => ({ ...s, [k]: !s[k] }));

  // changing model type regenerates the parameter + capability sets
  const setModality = (modality: Modality) =>
    setDraft((d) => {
      const caps = defaultCaps(modality);
      return { ...d, modality, caps, params: paramDefs(modality, caps.reasoning) };
    });
  // the reasoning capability adds/removes the reasoning_effort param
  const setReasoning = (on: boolean) =>
    setDraft((d) => {
      const caps = { ...d.caps, reasoning: on };
      const custom = d.params.filter((p) => p.custom);
      return { ...d, caps, params: [...paramDefs("chat", on), ...custom] };
    });

  const provider = providers.find((p) => p.id === draft.providerId) ?? null;
  const providerName = provider?.name ?? (mode === "view" ? "config" : "");

  // -- validation (verbose, blocks save) ------------------------------------
  const publicName = draft.alias.trim() || draft.upstreamName.trim();
  const errProvider =
    !readonly && !draft.providerId
      ? "Required. Pick the upstream provider that actually serves this model."
      : "";
  const errUpstream =
    !readonly && !draft.upstreamName.trim()
      ? "Required. Enter the model id exactly as the provider's API expects it — this is the string sent to the base URL (e.g. gpt-4o, claude-sonnet-4-20250514)."
      : "";
  const nameConflict =
    !readonly &&
    publicName !== "" &&
    models.some(
      (m) =>
        m.model.toLowerCase() === publicName.toLowerCase() &&
        (mode !== "edit" || m.model !== route?.model),
    );
  const errAlias = nameConflict
    ? `A model named “${publicName}” already exists. Give this one a distinct Rolter alias.`
    : "";
  const errBaseUrl =
    draft.baseUrl.trim() !== "" && !/^https?:\/\//i.test(draft.baseUrl.trim())
      ? "Base URL must start with http:// or https://. Leave blank to use the provider's default endpoint."
      : "";
  const errParam = draft.params.some(
    (p) => p.custom && p.value.trim() !== "" && p.key.trim() === "",
  )
    ? "One or more custom parameters have a value but no name — name them or clear the value."
    : "";
  const errHeader = draft.headers.some(
    (h) => h.value.trim() !== "" && h.key.trim() === "",
  )
    ? "One or more custom headers have a value but no name — name them or clear the value."
    : "";
  const errors = [errProvider, errUpstream, errAlias, errBaseUrl, errParam, errHeader].filter(
    Boolean,
  );
  const canSave = !readonly && !editLoading && errors.length === 0;
  // the one the footer repeats beside the disabled button; the summary above it
  // still lists the rest
  const blockingError = readonly ? "" : (errors[0] ?? "");
  const blockingErrorId = React.useId();

  // -- persistence ----------------------------------------------------------
  // route + first target, default params with the lock policy, the enabled
  // flag and pricing go through their own endpoints; the catalog metadata,
  // limits, headers and visibility travel together as the route's `advanced`
  // blob (#1189).
  // form lifecycle for the UX stream (#805); names the form, never its contents
  const ux = useFormTelemetry(mode === "add" ? "model-create" : "model-edit", open);
  // the advanced blob is the last write of the save, so a rejection there means
  // the rest already landed — the footer has to say which half failed rather
  // than print `validate_advanced`'s message with nothing around it
  const [advancedRejected, setAdvancedRejected] = React.useState(false);
  const advancedPayload = advancedToApi(draft, mode === "edit" ? (route?.advanced ?? {}) : {});
  const writeAdvanced = async (routeId: string) => {
    if (JSON.stringify(advancedPayload) === initialAdvancedRef.current) return;
    try {
      await setRouteAdvanced(routeId, advancedPayload);
    } catch (err) {
      setAdvancedRejected(true);
      throw err;
    }
  };

  const save = useMutation({
    mutationFn: async () => {
      setAdvancedRejected(false);
      const { params, paramPolicy } = paramsToApi(draft);
      const upstream = draft.upstreamName.trim();
      const hasPricing = draft.price.input.trim() !== "" || draft.price.output.trim() !== "";
      if (mode === "add") {
        const created = await createRoute(projectId as string, {
          model: publicName,
          strategy: STRATEGIES[0],
        });
        if (draft.providerId) {
          await createRouteTarget(created.id, {
            provider_id: draft.providerId,
            upstream_model: upstream !== publicName ? upstream : undefined,
            weight: Number(draft.net.weight) || 1,
          });
        }
        if (Object.keys(params).length > 0 || draft.paramMode !== "unlockAll") {
          await updateRouteParams(created.id, params, paramPolicy);
        }
        if (!draft.enabled) await setRouteEnabled(created.id, false);
        if (hasPricing) {
          await upsertModelPrice({
            model: publicName,
            input_per_mtok: draft.price.input.trim() || "0",
            output_per_mtok: draft.price.output.trim() || "0",
            cached_input_per_mtok: draft.price.cacheRead.trim() || undefined,
            currency: draft.price.currency,
          });
        }
        await writeAdvanced(created.id);
        return;
      }
      // edit
      const r = route!;
      await updateRouteParams(r.id, params, paramPolicy);
      if (draft.enabled !== r.enabled) await setRouteEnabled(r.id, draft.enabled);
      const target = targets.data?.[0];
      const wantUpstream = upstream !== r.model ? upstream : undefined;
      const weight = Number(draft.net.weight) || 1;
      const targetChanged =
        draft.providerId &&
        (!target ||
          target.provider_id !== draft.providerId ||
          (target.upstream_model ?? undefined) !== wantUpstream ||
          target.weight !== weight);
      if (targetChanged) {
        if (target) await deleteRouteTarget(target.id);
        await createRouteTarget(r.id, {
          provider_id: draft.providerId,
          upstream_model: wantUpstream,
          weight,
        });
      }
      if (hasPricing) {
        await upsertModelPrice({
          model: r.model,
          input_per_mtok: draft.price.input.trim() || "0",
          output_per_mtok: draft.price.output.trim() || "0",
          cached_input_per_mtok: draft.price.cacheRead.trim() || undefined,
          currency: draft.price.currency,
        });
      }
      await writeAdvanced(r.id);
    },
    onSuccess: () => {
      ux.saved();
      queryClient.invalidateQueries({ queryKey: ["route-targets", route?.id] });
      queryClient.invalidateQueries({ queryKey: ["model-prices"] });
      // the route list owns `route.advanced`, and the sheet seeds the editor
      // from it on the next open — a stale list would reopen on the values
      // this save just replaced
      queryClient.invalidateQueries({ queryKey: ["routes"] });
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        mode === "add"
          ? { tone: "success", title: t("toast.created", { what: publicName }) }
          : {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: publicName }),
            },
      );
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      ux.failed();
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: publicName }),
        detail: errorDetail(error),
      });
    },
  });

  const runTest = () => {
    setTestState("testing");
    window.setTimeout(() => setTestState("ok"), 1100);
  };

  // duplicate-from: prefill the draft from an existing db route, then tweak
  const applyDupFrom = (routeId: string) => {
    setDupFrom(routeId);
    if (!routeId) return;
    const src = routes.find((r) => r.id === routeId);
    if (!src) return;
    setDraft((d) => {
      const next = blankDraft(d.providerId || providers[0]?.id || "");
      next.upstreamName = src.model;
      next.alias = "";
      next.enabled = src.enabled;
      seedParams(next, src.params ?? {}, src.param_policy ?? {});
      return next;
    });
  };

  const title = mode === "add" ? "Add model" : readonly ? "Model details" : "Edit model";
  const subtitle =
    mode === "add"
      ? "Register a model and how the gateway calls it."
      : `${providerName || "—"} · ${draft.upstreamName.trim() || "—"}`;
  const cta = mode === "add" ? "Add model" : "Save model";

  const showCaps = draft.modality === "chat" || draft.modality === "audio";
  const cur = draft.price.currency;
  // a stored price may name a code the rate table no longer carries; keep it
  // selectable so saving an unrelated field cannot silently re-denominate it
  const currencyOptions = React.useMemo(() => {
    const codes = currency.data?.codes ?? [];
    const offered = codes.length > 0 ? codes : [cur || "USD"];
    return offered.some((c) => c.toUpperCase() === cur.trim().toUpperCase())
      ? offered
      : [...offered, cur];
  }, [currency.data, cur]);
  const currencyUnconvertible = !isConvertible(currency.data, cur);
  const paramManual = draft.paramMode === "manual";
  const headerManual = draft.headerMode === "manual";
  const modeNote =
    draft.paramMode === "lockAll"
      ? "Locked: clients cannot override any parameter — these server-side values are enforced."
      : draft.paramMode === "unlockAll"
        ? "Open: clients may override any parameter. Values here act as defaults only."
        : "Manual: lock individual parameters below; unlocked ones stay client-overridable.";

  const lockModeOptions: { value: LockMode; label: string }[] = [
    { value: "lockAll", label: "Lock all" },
    { value: "unlockAll", label: "Unlock all" },
    { value: "manual", label: "Manual" },
  ];

  const numInput = (
    key: keyof ModelDraft["net"],
    label: string,
    placeholder: string,
    info?: string,
  ) => (
    <div className="space-y-1">
      <FieldLabel label={label} info={info} htmlFor={`ms-net-${key}`} />
      <Input
        id={`ms-net-${key}`}
        type="number"
        className="font-mono"
        value={draft.net[key] as string}
        placeholder={placeholder}
        disabled={readonly}
        onChange={(e) => setDeep("net", { [key]: e.target.value } as Partial<ModelDraft["net"]>)}
      />
    </div>
  );

  const priceInput = (key: "input" | "output" | "cacheWrite" | "cacheRead", label: string) => (
    <div className="space-y-1">
      <FieldLabel label={label} htmlFor={`ms-price-${key}`} />
      <Input
        id={`ms-price-${key}`}
        type="number"
        step="any"
        className="font-mono"
        value={draft.price[key]}
        placeholder="0.00"
        disabled={readonly}
        onChange={(e) => setDeep("price", { [key]: e.target.value })}
      />
    </div>
  );

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader
        title={title}
        subtitle={subtitle}
        onClose={() => guard() && onOpenChange(false)}
      />
      <SheetBody>
        {readonly && (
          <div className="flex items-start gap-2.5 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-3 py-2.5">
            <Lock className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--text-secondary)]" />
            <p className="text-xs leading-snug text-[color:var(--text-secondary)]">
              Read-only config model — defined in config and always present. Fields are
              shown for reference; edits and deletes are rejected with{" "}
              <span className="font-mono text-foreground">409 Conflict</span>.
            </p>
          </div>
        )}
        {editLoading && <FormSkeleton fields={3} />}

        {mode === "add" && (
          <div className="space-y-1.5">
            <FieldLabel
              label="Duplicate from"
              info="Prefill every field from an existing model, then tweak. Handy for adding a second deployment of the same model on another provider."
              htmlFor="ms-field-1"
            />
            <Select
              id="ms-field-1"
              className="font-mono"
              value={dupFrom}
              onChange={(e) => applyDupFrom(e.target.value)}
            >
              <option value="">Start from scratch…</option>
              {routes.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.model}
                </option>
              ))}
            </Select>
          </div>
        )}

        {/* ===== General ===== */}
        <Section
          title="General"
          open={secOpen.general}
          onToggle={() => toggleSec("general")}
          className="space-y-3.5"
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div className="space-y-1.5">
              <FieldLabel
                label="Provider"
                required
                info="The upstream provider that serves this model. Sets auth, endpoint shape, and which parameters are valid."
                htmlFor="ms-field-2"
              />
              <Select
                id="ms-field-2"
                className="font-mono"
                value={draft.providerId}
                disabled={readonly}
                onChange={(e) => set({ providerId: e.target.value })}
              >
                <option value="">{readonly ? "config" : "select provider…"}</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Select>
              <FieldError error={errProvider} />
            </div>
            <div className="space-y-1.5">
              <FieldLabel
                label="Model type"
                info="The modality this endpoint handles. Determines which request schema and playground surface apply."
                htmlFor="ms-field-3"
              />
              <Select
                id="ms-field-3"
                className="font-mono"
                value={draft.modality}
                disabled={readonly}
                onChange={(e) => setModality(e.target.value as Modality)}
              >
                {MODALITIES.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label="Upstream model name"
              required
              info="The exact model id the provider expects — this string is sent to the base URL. e.g. gpt-4o, claude-sonnet-4-20250514, Llama-3.1-8B-Instruct."
              htmlFor="ms-field-4"
            />
            <Input
              id="ms-field-4"
              className="font-mono"
              value={draft.upstreamName}
              placeholder="gpt-4o"
              disabled={readonly || mode === "edit"}
              onChange={(e) => set({ upstreamName: e.target.value })}
            />
            <p className="text-xs text-muted-foreground">
              {mode === "edit"
                ? "Renaming isn't supported yet — delete and re-add to change the name."
                : "Sent verbatim to the provider — must match their API exactly."}
            </p>
            <FieldError error={errUpstream} />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label="Rolter alias"
              info="Optional. The public name clients call this model by. Leave blank to reuse the upstream name. Use it to expose a stable, provider-agnostic name."
              htmlFor="ms-field-5"
            />
            <Input
              id="ms-field-5"
              className="font-mono"
              value={draft.alias}
              placeholder={draft.upstreamName.trim() || "same as upstream name"}
              disabled={readonly || mode === "edit"}
              onChange={(e) => set({ alias: e.target.value })}
            />
            <p className="text-xs text-muted-foreground">
              The name clients send. Optional — defaults to the upstream name.
            </p>
            <FieldError error={errAlias} />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label="Base URL override"
              info="Point this model at a custom endpoint (self-hosted, proxy, or region). Leave blank to use the provider's default base URL."
              htmlFor="ms-field-6"
            />
            <Input
              id="ms-field-6"
              className="font-mono"
              value={draft.baseUrl}
              placeholder="https://api.provider.com/v1"
              disabled={readonly}
              onChange={(e) => set({ baseUrl: e.target.value })}
            />
            <p className="text-xs text-muted-foreground">
              Optional. Overrides the provider endpoint for this model only.
            </p>
            <FieldError error={errBaseUrl} />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label="Description"
              info="Free text shown in the catalog and pickers. Note capabilities, intended use, or gotchas for your team."
              htmlFor="ms-field-7"
            />
            <Textarea
              id="ms-field-7"
              value={draft.description}
              placeholder="What is this model for? Any routing notes for the team…"
              disabled={readonly}
              onChange={(e) => set({ description: e.target.value })}
            />
          </div>
          <SwitchRow
            title="Enabled"
            hint="Off = kept in the catalog but excluded from routing and pickers."
            checked={draft.enabled}
            disabled={readonly}
            onChange={(v) => set({ enabled: v })}
          />
        </Section>

        {/* ===== Default parameters ===== */}
        <Section
          title="Default parameters"
          info="Server-side default values for inference parameters, and whether clients may override each one."
          open={secOpen.params}
          onToggle={() => toggleSec("params")}
          className="space-y-3"
        >
          <Segmented
            ariaLabel={t("modelSheet.paramLockMode")}
            value={draft.paramMode}
            options={lockModeOptions}
            disabled={readonly}
            onChange={(v) => set({ paramMode: v })}
          />
          <p className="text-xs leading-snug text-muted-foreground">{modeNote}</p>
          <div className="space-y-2">
            {draft.params.map((p, i) => (
              <div key={p.custom ? `c${i}` : p.key} className="flex items-center gap-2">
                {p.custom ? (
                  <Input
                    aria-label="Param name"
                    className="h-[34px] flex-[1.1] font-mono text-xs"
                    value={p.key}
                    placeholder="param name"
                    disabled={readonly}
                    onChange={(e) => setParamAt(i, { key: e.target.value })}
                  />
                ) : (
                  <span className="min-w-0 flex-[1.1] truncate font-mono text-sm">
                    {p.key}
                  </span>
                )}
                {p.type === "enum" ? (
                  <Select
                    aria-label="Param value"
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    value={p.value}
                    disabled={readonly}
                    onChange={(e) => setParamAt(i, { value: e.target.value })}
                  >
                    {(p.opts ?? ["", "low", "medium", "high"]).map((o) => (
                      <option key={o} value={o}>
                        {o === "" ? "provider default" : o}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    aria-label="Param value"
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    type={p.type === "int" || p.type === "float" ? "number" : "text"}
                    step="any"
                    value={p.value}
                    placeholder={p.custom ? "value" : "provider default"}
                    disabled={readonly}
                    onChange={(e) => setParamAt(i, { value: e.target.value })}
                  />
                )}
                {p.custom && (
                  <Select
                    aria-label="Param type"
                    className="h-[34px] w-20 flex-none font-mono text-[11px]"
                    value={p.type}
                    disabled={readonly}
                    onChange={(e) => setParamAt(i, { type: e.target.value as ParamType })}
                  >
                    {PARAM_TYPES.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </Select>
                )}
                {paramManual && (
                  <LockButton
                    locked={p.locked}
                    disabled={readonly}
                    onToggle={() => setParamAt(i, { locked: !p.locked })}
                  />
                )}
                {p.custom && (
                  <button
                    type="button"
                    title="Remove"
                    aria-label="Remove param"
                    disabled={readonly}
                    onClick={() =>
                      setDraft((d) => ({
                        ...d,
                        params: d.params.filter((_, idx) => idx !== i),
                      }))
                    }
                    className="flex flex-none rounded-md border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-subtle)] transition-colors hover:border-destructive hover:text-[color:var(--status-danger-text)]"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            ))}
          </div>
          <FieldError error={errParam} />
          {!readonly && (
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setDraft((d) => ({
                  ...d,
                  params: [
                    ...d.params,
                    {
                      key: "",
                      value: "",
                      type: "string",
                      locked: d.paramMode === "lockAll",
                      custom: true,
                    },
                  ],
                }))
              }
            >
              <Plus className="h-3.5 w-3.5" />
              Add parameter
            </Button>
          )}
        </Section>

        {/* ===== Capabilities (chat + audio only) ===== */}
        {showCaps && (
          <Section
            title="Capabilities"
            info="What this model endpoint supports. Flags gate request features and hint the playground — they don't add capabilities the provider lacks. The set shown depends on the model type."
            open={secOpen.caps}
            onToggle={() => toggleSec("caps")}
            className="grid grid-cols-1 gap-2.5 sm:grid-cols-2"
          >
            <SwitchRow
              title="Streaming"
              checked={draft.caps.streaming}
              disabled={readonly}
              onChange={(v) => setDeep("caps", { streaming: v })}
            />
            {draft.modality === "chat" && (
              <>
                <SwitchRow
                  title="Tools / functions"
                  checked={draft.caps.tools}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { tools: v })}
                />
                <SwitchRow
                  title="Vision / images"
                  checked={draft.caps.vision}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { vision: v })}
                />
                <SwitchRow
                  title="JSON mode"
                  checked={draft.caps.json}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { json: v })}
                />
                <SwitchRow
                  title="Reasoning"
                  info="Extended-thinking model (o-series, R1). Enables the reasoning_effort parameter."
                  checked={draft.caps.reasoning}
                  disabled={readonly}
                  onChange={setReasoning}
                />
              </>
            )}
          </Section>
        )}

        {/* ===== Pricing override ===== */}
        <Section
          title="Pricing override"
          info="Override the datasheet price for accurate cost tracking. All fields optional — blank counts as 0."
          open={secOpen.pricing}
          onToggle={() => toggleSec("pricing")}
          className="space-y-3"
        >
          <p className="text-xs text-muted-foreground">
            Optional cost overrides for accurate tracking — fields shown match the model
            type. Leave blank for free / provider-tracked.
          </p>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {(draft.modality === "chat" || draft.modality === "embedding") &&
              priceInput("input", `Input ${cur}/Mtok`)}
            {draft.modality === "chat" && (
              <>
                {priceInput("output", `Output ${cur}/Mtok`)}
                {priceInput("cacheWrite", `Cache-write ${cur}/Mtok`)}
                {priceInput("cacheRead", `Cache-read ${cur}/Mtok`)}
              </>
            )}
          </div>
          {(draft.modality === "image" || draft.modality === "audio") && (
            <div className="space-y-1">
              <FieldLabel
                label={
                  draft.modality === "image"
                    ? `Flat price per image (${cur})`
                    : `Flat price per minute (${cur})`
                }
                htmlFor="ms-field-8"
              />
              <Input
                id="ms-field-8"
                type="number"
                step="any"
                className="font-mono"
                value={draft.price.perRequest}
                placeholder="0.00"
                disabled={readonly}
                onChange={(e) => setDeep("price", { perRequest: e.target.value })}
              />
            </div>
          )}
          <div className="flex items-end gap-3">
            <div className="w-36 space-y-1">
              <FieldLabel label="Currency" htmlFor="ms-field-9" />
              <Select
                id="ms-field-9"
                className="font-mono"
                value={cur}
                disabled={readonly}
                onChange={(e) => setDeep("price", { currency: e.target.value })}
              >
                {currencyOptions.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </Select>
            </div>
            <a
              href={PRICING_DOCS_URL}
              target="_blank"
              rel="noreferrer"
              className="pb-2 text-xs text-muted-foreground hover:text-foreground"
            >
              {t("modelSheet.pricingDocs")}
            </a>
          </div>
          {currencyUnconvertible && (
            <p className="text-xs text-[color:var(--status-warning-text)]">
              {t("modelSheet.currencyUnconvertible", {
                code: cur,
                base: currency.data?.base ?? "",
              })}
            </p>
          )}
        </Section>

        {/* ===== Limits & network ===== */}
        <Section
          title="Limits & network"
          open={secOpen.advanced}
          onToggle={() => toggleSec("advanced")}
          className="space-y-3"
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {numInput(
              "rpm",
              "Requests / min",
              "unlimited",
              "Max requests per minute to this model. Blank = no per-model cap (virtual-key limits still apply).",
            )}
            {numInput(
              "tpm",
              "Tokens / min",
              "unlimited",
              "Max tokens per minute across requests to this model. Blank = no per-model cap.",
            )}
            {numInput("concurrency", "Max concurrency", "unlimited")}
            {numInput("timeoutMs", "Timeout (ms)", "30000")}
            {numInput("retries", "Max retries", "2")}
            {numInput(
              "weight",
              "Routing weight",
              "100",
              "Relative share of traffic when this model is one of several targets for the same alias.",
            )}
            {numInput("context", "Context window", "128000")}
            {numInput("maxOutput", "Max output tokens", "16384")}
          </div>
          <SwitchRow
            title="Allow insecure TLS"
            hint="Disables cert verification for this model's endpoint."
            info="Skip TLS certificate verification. Only for self-signed or private-CA endpoints you trust — never for public providers."
            checked={draft.net.insecureTls}
            disabled={readonly}
            onChange={(v) => setDeep("net", { insecureTls: v })}
          />
          <SwitchRow
            title="Allow additional fields"
            hint="Forward unknown fields instead of stripping them."
            info="Pass through request fields not in Rolter's schema straight to the provider — for provider-specific options Rolter doesn't model yet."
            checked={draft.net.allowAdditional}
            disabled={readonly}
            onChange={(v) => setDeep("net", { allowAdditional: v })}
          />
        </Section>

        {/* ===== Custom request headers ===== */}
        <Section
          title="Custom request headers"
          info="Extra HTTP headers sent upstream with every request. Same lock rules as parameters — control whether clients can override them."
          open={secOpen.headers}
          onToggle={() => toggleSec("headers")}
          className="space-y-3"
        >
          <Segmented
            ariaLabel={t("modelSheet.headerLockMode")}
            value={draft.headerMode}
            options={lockModeOptions}
            disabled={readonly}
            onChange={(v) => set({ headerMode: v })}
          />
          {draft.headers.length > 0 && (
            <div className="space-y-2">
              {draft.headers.map((h, i) => (
                <div key={i} className="flex items-center gap-2">
                  <Input
                    aria-label="Header name"
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    value={h.key}
                    placeholder="Header-Name"
                    disabled={readonly}
                    onChange={(e) => setHeaderAt(i, { key: e.target.value })}
                  />
                  <Input
                    aria-label="Header value"
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    value={h.value}
                    placeholder="value"
                    disabled={readonly}
                    onChange={(e) => setHeaderAt(i, { value: e.target.value })}
                  />
                  {headerManual && (
                    <LockButton
                      locked={h.locked}
                      disabled={readonly}
                      onToggle={() => setHeaderAt(i, { locked: !h.locked })}
                    />
                  )}
                  <button
                    type="button"
                    title="Remove"
                    aria-label="Remove header"
                    disabled={readonly}
                    onClick={() =>
                      setDraft((d) => ({
                        ...d,
                        headers: d.headers.filter((_, idx) => idx !== i),
                      }))
                    }
                    className="flex flex-none rounded-md border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-subtle)] transition-colors hover:border-destructive hover:text-[color:var(--status-danger-text)]"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
              ))}
            </div>
          )}
          <FieldError error={errHeader} />
          {!readonly && (
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setDraft((d) => ({
                  ...d,
                  headers: [
                    ...d.headers,
                    { key: "", value: "", locked: d.headerMode === "lockAll" },
                  ],
                }))
              }
            >
              <Plus className="h-3.5 w-3.5" />
              Add header
            </Button>
          )}
        </Section>

        {/* ===== Access & permissions ===== */}
        <Section
          title="Access & permissions"
          open={secOpen.rbac}
          onToggle={() => toggleSec("rbac")}
          className="space-y-3.5"
        >
          <div className="space-y-1.5">
            <FieldLabel
              label="Minimum role"
              info="The lowest role allowed to call this model. Members below this role won't see it in pickers or be able to invoke it."
              htmlFor="ms-field-10"
            />
            <Select
              id="ms-field-10"
              className="font-mono"
              value={draft.rbac.minRole}
              disabled={readonly}
              onChange={(e) => setDeep("rbac", { minRole: e.target.value })}
            >
              {ROLES.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </Select>
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label="Visibility"
              info="Public = available to anyone meeting the minimum role. Restricted = only the teams, virtual keys, and users you list below."
              id="ms-visibility-label"
            />
            <Segmented
              labelledBy="ms-visibility-label"
              value={draft.rbac.visibility}
              options={[
                { value: "public", label: "Public" },
                { value: "restricted", label: "Restricted" },
              ]}
              disabled={readonly}
              onChange={(v) => setDeep("rbac", { visibility: v })}
            />
          </div>
          {draft.rbac.visibility === "restricted" && (
            <div className="space-y-3.5">
              <ChipGroup
                label="Allowed teams / business units"
                options={(teams.data ?? []).map((row) => ({ id: row.id, name: row.name }))}
                selected={draft.rbac.teams}
                disabled={readonly}
                onToggle={(v) =>
                  setDeep("rbac", {
                    teams: draft.rbac.teams.includes(v)
                      ? draft.rbac.teams.filter((x) => x !== v)
                      : [...draft.rbac.teams, v],
                  })
                }
              />
              <ChipGroup
                label="Allowed virtual keys"
                options={(vkeys.data ?? []).map((row) => ({
                  id: row.id,
                  name: row.name || row.key_prefix,
                }))}
                selected={draft.rbac.vkeys}
                disabled={readonly}
                onToggle={(v) =>
                  setDeep("rbac", {
                    vkeys: draft.rbac.vkeys.includes(v)
                      ? draft.rbac.vkeys.filter((x) => x !== v)
                      : [...draft.rbac.vkeys, v],
                  })
                }
              />
              <ChipGroup
                label="Restrict to specific users"
                options={(users.data ?? []).map((row) => ({ id: row.id, name: row.email }))}
                selected={draft.rbac.users}
                disabled={readonly}
                onToggle={(v) =>
                  setDeep("rbac", {
                    users: draft.rbac.users.includes(v)
                      ? draft.rbac.users.filter((x) => x !== v)
                      : [...draft.rbac.users, v],
                  })
                }
              />
            </div>
          )}
        </Section>

        {/* ===== Config preview ===== */}
        <Section
          title="Config preview"
          open={secOpen.preview}
          onToggle={() => toggleSec("preview")}
        >
          {/* the draft as the config it will become, through the shared code
              block so a mistyped key or a stray quote shows up here rather
              than after saving (#949) */}
          <CodeBlock
            value={buildPreview(draft, providerName, advancedPayload)}
            language="json"
            label={t("modelSheet.configPreview")}
            maxHeight={280}
            density="compact"
          />
        </Section>
      </SheetBody>

      <SheetFooter>
        {errors.length > 0 && (
          <div className="space-y-1 px-[22px] pt-2.5">
            {errors.map((e) => (
              <p key={e} className="text-xs leading-snug text-[color:var(--status-danger-text)]">
                • {e}
              </p>
            ))}
          </div>
        )}
        {save.isError && (
          <p role="alert" className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">
            {advancedRejected
              ? t("modelSheet.advancedRejected", { message: (save.error as Error).message })
              : (save.error as Error).message}
          </p>
        )}
        <div className="flex items-center gap-2.5 px-[22px] py-3.5">
          <button
            type="button"
            disabled={readonly}
            onClick={runTest}
            className={cn(
              "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              "inline-flex h-9 items-center gap-1.5 rounded-md border border-[color:var(--border-subtle)] px-3 text-sm transition-colors hover:bg-[color:var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-50",
              testState === "ok"
                ? "text-[color:var(--status-success-text)]"
                : "text-[color:var(--text-secondary)]",
            )}
          >
            {testState === "ok" ? (
              <Check className="h-[15px] w-[15px]" />
            ) : (
              <Plug className="h-[15px] w-[15px]" />
            )}
            {testState === "testing"
              ? "Testing…"
              : testState === "ok"
                ? "Connection OK"
                : "Test connection"}
          </button>
          {/* the primary action stays where it is and greys out instead of
              vanishing (#1265): a footer that reflows tells an operator who
              never scrolled to the field errors only that saving is gone, so
              the first error travels with the button and names the reason */}
          {blockingError && (
            <p
              id={blockingErrorId}
              role="alert"
              className="ml-auto max-w-[52%] text-right text-xs leading-snug text-[color:var(--status-danger-text)]"
            >
              {blockingError}
            </p>
          )}
          <span className={cn("inline-flex gap-2.5", !blockingError && "ml-auto")}>
            <Button variant="ghost" onClick={() => guard() && onOpenChange(false)}>
              Cancel
            </Button>
            {readonly && (
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
            )}
            {!readonly && (
              <Button
                disabled={!canSave || save.isPending}
                aria-describedby={blockingError ? blockingErrorId : undefined}
                onClick={() => {
                  ux.submitted();
                  save.mutate();
                }}
              >
                {cta}
              </Button>
            )}
          </span>
        </div>
      </SheetFooter>
    </Sheet>
  );
}
