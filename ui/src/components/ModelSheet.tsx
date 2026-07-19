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

import { Button } from "@/components/ui/button";
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
  fetchModelPrices,
  fetchRouteTargets,
  fetchTeams,
  fetchUsers,
  fetchVirtualKeys,
  ROLES,
  setRouteEnabled,
  STRATEGIES,
  updateRouteParams,
  upsertModelPrice,
  type EffectiveModelDto,
  type ProviderRow,
  type RouteRow,
} from "@/lib/api";
import { cn } from "@/lib/utils";

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

const MODALITIES: Modality[] = ["chat", "embedding", "image", "audio"];
const CURRENCIES = ["USD", "EUR", "GBP", "INR", "JPY", "CAD", "AUD"];
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

// live JSON of the resulting model entry — only non-empty fields
function buildPreview(draft: ModelDraft, providerName: string) {
  const num = (v: string) => {
    const n = parseFloat(v);
    return Number.isNaN(n) ? undefined : n;
  };
  const paramSet = draft.params
    .filter((p) => p.key.trim() && p.value.trim() !== "")
    .map((p) => ({
      key: p.key.trim(),
      value: coerce(p.value, p.type),
      locked: effLock(draft.paramMode, p.locked),
    }));
  const pricing: Record<string, unknown> = {};
  (["input", "output", "cacheWrite", "cacheRead", "perRequest"] as const).forEach((k) => {
    const n = num(draft.price[k]);
    if (n) pricing[k] = n;
  });
  const network: Record<string, unknown> = {};
  if (draft.net.insecureTls) network.allow_insecure_tls = true;
  (["rpm", "tpm", "concurrency", "timeoutMs", "retries", "weight", "context", "maxOutput"] as const).forEach(
    (k) => {
      const n = num(draft.net[k]);
      if (n != null) network[k] = n;
    },
  );
  const headers = draft.headers
    .filter((h) => h.key.trim())
    .map((h) => ({
      key: h.key.trim(),
      value: h.value,
      locked: effLock(draft.headerMode, h.locked),
    }));
  if (headers.length) network.custom_headers = { mode: draft.headerMode, values: headers };
  const obj = {
    provider: providerName || undefined,
    model: draft.upstreamName.trim() || undefined,
    alias: draft.alias.trim() || undefined,
    type: draft.modality,
    enabled: draft.enabled,
    base_url: draft.baseUrl.trim() || undefined,
    description: draft.description.trim() || undefined,
    params: paramSet.length ? { mode: draft.paramMode, values: paramSet } : undefined,
    allow_additional_fields: draft.net.allowAdditional || undefined,
    capabilities: Object.entries(draft.caps)
      .filter(([, v]) => v)
      .map(([k]) => k),
    pricing: Object.keys(pricing).length
      ? { ...pricing, currency: draft.price.currency }
      : undefined,
    network: Object.keys(network).length ? network : undefined,
    access:
      draft.rbac.visibility === "restricted"
        ? {
            min_role: draft.rbac.minRole,
            visibility: "restricted",
            teams: draft.rbac.teams,
            virtual_keys: draft.rbac.vkeys,
            users: draft.rbac.users,
          }
        : { min_role: draft.rbac.minRole, visibility: "public" },
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
}: {
  label: string;
  required?: boolean;
  info?: string;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <label className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </label>
      {required && <span className="text-xs text-destructive">*</span>}
      {info && <InfoHint text={info} label={`About ${label}`} />}
    </div>
  );
}

function FieldError({ error }: { error?: string }) {
  if (!error) return null;
  return <p className="text-xs leading-snug text-destructive">{error}</p>;
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
      {/* a div, not a button: the InfoHint inside is itself a button and
          nested buttons are invalid HTML (the browser re-parents them) */}
      <div
        role="button"
        tabIndex={0}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        aria-expanded={open}
        className="flex w-full cursor-pointer items-center gap-2.5 px-[15px] py-[13px] text-left"
      >
        <span className="text-sm font-semibold">{title}</span>
        {info && (
          <span onClick={(e) => e.stopPropagation()}>
            <InfoHint text={info} label={`About ${title}`} />
          </span>
        )}
        <ChevronDown
          className={cn(
            "ml-auto h-4 w-4 text-[color:var(--text-subtle)] transition-transform duration-[120ms]",
            open && "rotate-180",
          )}
        />
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
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  return (
    <div className="inline-flex w-fit rounded-md bg-[color:var(--surface-subtle)] p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          disabled={disabled}
          onClick={() => onChange(o.value)}
          className={cn(
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
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onToggle}
      aria-pressed={locked}
      title={
        locked
          ? "Locked — clients can't override. Click to unlock."
          : "Unlocked — clients can override. Click to lock."
      }
      className={cn(
        "flex h-8 w-8 flex-none items-center justify-center rounded-md border transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
        locked
          ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-[color:var(--red-500)]"
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
  options: string[];
  selected: string[];
  onToggle: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="space-y-1.5">
      <label className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </label>
      <div className="flex flex-wrap gap-1.5">
        {options.length === 0 && (
          <p className="text-xs text-muted-foreground">none available</p>
        )}
        {options.map((v) => {
          const on = selected.includes(v);
          return (
            <button
              key={v}
              type="button"
              disabled={disabled}
              onClick={() => onToggle(v)}
              aria-pressed={on}
              className={cn(
                "inline-flex h-7 items-center rounded-full border px-2.5 font-mono text-xs transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
                on
                  ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-foreground"
                  : "border-[color:var(--border-subtle)] bg-transparent text-muted-foreground",
              )}
            >
              {v}
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
      <Switch checked={checked} onCheckedChange={onChange} disabled={disabled} />
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
  }, [open, mode, route, configModel, providers, targets.data, prices.data, editLoading]);

  const dirty = !readonly && initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const guard = React.useCallback(() => {
    if (!dirty) return true;
    return window.confirm("Discard unsaved changes?");
  }, [dirty]);

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

  // -- persistence ----------------------------------------------------------
  // wires the fields the control api models today: route + first target,
  // default params with the lock policy, enabled flag, and pricing. the
  // capability / network / header / rbac sections are forward-looking — they
  // render and preview but have no backing DTOs yet.
  const save = useMutation({
    mutationFn: async () => {
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
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["route-targets", route?.id] });
      queryClient.invalidateQueries({ queryKey: ["model-prices"] });
      onDone();
      onOpenChange(false);
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
      <FieldLabel label={label} info={info} />
      <Input
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
      <FieldLabel label={label} />
      <Input
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
        {editLoading && <p className="text-xs text-muted-foreground">Loading…</p>}

        {mode === "add" && (
          <div className="space-y-1.5">
            <FieldLabel
              label="Duplicate from"
              info="Prefill every field from an existing model, then tweak. Handy for adding a second deployment of the same model on another provider."
            />
            <Select
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
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <FieldLabel
                label="Provider"
                required
                info="The upstream provider that serves this model. Sets auth, endpoint shape, and which parameters are valid."
              />
              <Select
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
              />
              <Select
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
            />
            <Input
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
            />
            <Input
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
            />
            <Input
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
            />
            <Textarea
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
                    className="flex flex-none rounded-md border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-subtle)] transition-colors hover:border-destructive hover:text-destructive"
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
            className="grid grid-cols-2 gap-2.5"
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
          <div className="grid grid-cols-2 gap-3">
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
              />
              <Input
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
              <FieldLabel label="Currency" />
              <Select
                className="font-mono"
                value={cur}
                disabled={readonly}
                onChange={(e) => setDeep("price", { currency: e.target.value })}
              >
                {CURRENCIES.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </Select>
            </div>
            <a
              href="https://getbifrost.ai/datasheet"
              target="_blank"
              rel="noreferrer"
              className="pb-2 text-xs text-muted-foreground hover:text-foreground"
            >
              View pricing source ↗
            </a>
          </div>
        </Section>

        {/* ===== Limits & network ===== */}
        <Section
          title="Limits & network"
          open={secOpen.advanced}
          onToggle={() => toggleSec("advanced")}
          className="space-y-3"
        >
          <div className="grid grid-cols-2 gap-3">
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
                    className="flex flex-none rounded-md border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-subtle)] transition-colors hover:border-destructive hover:text-destructive"
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
            />
            <Select
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
            />
            <Segmented
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
                options={(teams.data ?? []).map((t) => t.name)}
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
                options={(vkeys.data ?? [])
                  .map((k) => k.name)
                  .filter((n): n is string => !!n)}
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
                options={(users.data ?? []).map((u) => u.email)}
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
          <pre className="max-h-[280px] overflow-auto rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 font-mono text-[11px] leading-relaxed text-[color:var(--text-secondary)]">
            {buildPreview(draft, providerName)}
          </pre>
        </Section>
      </SheetBody>

      <SheetFooter>
        {errors.length > 0 && (
          <div className="space-y-1 px-[22px] pt-2.5">
            {errors.map((e) => (
              <p key={e} className="text-xs leading-snug text-destructive">
                • {e}
              </p>
            ))}
          </div>
        )}
        {save.isError && (
          <p className="px-[22px] pt-2.5 text-xs text-destructive">
            {(save.error as Error).message}
          </p>
        )}
        <div className="flex items-center gap-2.5 px-[22px] py-3.5">
          <button
            type="button"
            disabled={readonly}
            onClick={runTest}
            className={cn(
              "inline-flex h-9 items-center gap-1.5 rounded-md border border-[color:var(--border-subtle)] px-3 text-sm transition-colors hover:bg-[color:var(--surface-hover)] disabled:cursor-not-allowed disabled:opacity-50",
              testState === "ok"
                ? "text-[color:var(--status-success)]"
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
          <span className="ml-auto inline-flex gap-2.5">
            <Button variant="ghost" onClick={() => guard() && onOpenChange(false)}>
              Cancel
            </Button>
            {readonly && (
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
            )}
            {canSave && (
              <Button disabled={save.isPending} onClick={() => save.mutate()}>
                {cta}
              </Button>
            )}
          </span>
        </div>
      </SheetFooter>
    </Sheet>
  );
}
