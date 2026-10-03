import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, Plus } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { FormSkeleton } from "@/components/LoadingState";
import { RouteTargetList } from "@/components/RouteTargetList";
import { StrategyHint } from "@/components/StrategyHint";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { Button } from "@/components/ui/button";
import { ChipGroup } from "@/components/ui/chip-group";
import { CodeBlock } from "@/components/ui/code-block";
import { Combobox } from "@/components/ui/combobox";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { FormSection } from "@/components/ui/form-section";
import { Input } from "@/components/ui/input";
import { LockButton } from "@/components/ui/lock-button";
import { Segmented } from "@/components/ui/segmented";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
import { SwitchRow } from "@/components/ui/switch-row";
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
  type RouteTargetRow,
} from "@/lib/api";
import type { RouteTargetView } from "@/lib/route-targets";
import { strategyOptions, usesWeights } from "@/lib/strategies";
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

/**
 * One target of the route, as the sheet edits it (#1979).
 *
 * `id` is the stored row this line was seeded from and is absent on a line
 * added in the sheet; `key` only keeps React's rows stable while lines are
 * added and removed.
 */
interface DraftTarget {
  key: string;
  id?: string;
  providerId: string;
  /** the model id sent upstream; blank sends the public name through as-is */
  upstream: string;
  weight: string;
}

let targetKeys = 0;
function newTarget(providerId: string, upstream = "", weight = "1", id?: string): DraftTarget {
  targetKeys += 1;
  return { key: `t${targetKeys}`, id, providerId, upstream, weight };
}

interface Caps {
  streaming: boolean;
  tools: boolean;
  vision: boolean;
  json: boolean;
  reasoning: boolean;
}

interface ModelDraft {
  /** the public name clients send in `model`, which is the route's name */
  name: string;
  strategy: string;
  targets: DraftTarget[];
  modality: Modality;
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
    rpm: string;
    tpm: string;
    concurrency: string;
    timeoutMs: string;
    retries: string;
    context: string;
    maxOutput: string;
  };
  headerMode: LockMode;
  headers: DraftHeader[];
  rbac: {
    minRole: string;
    visibility: "public" | "project" | "restricted";
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
  "https://github.com/rolter-ai/rolter/blob/master/docs/user-docs/observability/logs-and-cost.mdx#cost-tracking";

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
    name: "",
    strategy: STRATEGIES[0],
    // one line to start from, on the first provider: the common case is one
    // model on one provider, and a fleet adds lines from there
    targets: providerId ? [newTarget(providerId)] : [],
    modality: "chat",
    baseUrl: "",
    description: "",
    enabled: true,
    paramMode: "manual",
    params: paramDefs("chat", false),
    caps: defaultCaps("chat"),
    price: {
      input: "",
      output: "",
      cacheWrite: "",
      cacheRead: "",
      perRequest: "",
      currency: "USD",
    },
    net: {
      insecureTls: false,
      rpm: "",
      tpm: "",
      concurrency: "",
      timeoutMs: "",
      retries: "",
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

  const locked = new Set(strings(advanced.locked_headers));
  draft.headers = Object.entries(obj(advanced.headers)).map(([key, value]) => ({
    key,
    value: typeof value === "string" ? value : String(value),
    locked: locked.has(key),
  }));
  // "every header locked" and "none locked" are the two the sheet can round-trip
  // exactly; anything in between is the manual mode it already has for that
  draft.headerMode =
    draft.headers.length === 0
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
      : visibility.project_only === true
        ? "project"
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
    v && typeof v === "object" && !Array.isArray(v) ? { ...(v as Record<string, unknown>) } : {};
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
  // a model open to the whole organization or its project carries none of them.
  // `project_only` is sent only when set, so a save that never touched
  // visibility leaves the blob as it was — and a restricted route the API also
  // pinned to its project keeps the pin instead of quietly widening
  const restricted = draft.rbac.visibility === "restricted";
  const was = obj(stored.visibility);
  const wasListed = ["allowed_team_ids", "allowed_key_ids", "allowed_user_ids"].some(
    (key) => Array.isArray(was[key]) && (was[key] as unknown[]).length > 0,
  );
  const projectOnly =
    draft.rbac.visibility === "project" || (restricted && wasListed && was.project_only === true);
  out.visibility = {
    minimum_role: draft.rbac.minRole,
    allowed_team_ids: restricted ? draft.rbac.teams : [],
    allowed_key_ids: restricted ? draft.rbac.vkeys : [],
    allowed_user_ids: restricted ? draft.rbac.users : [],
    ...(projectOnly ? { project_only: true } : {}),
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
  draft.paramMode = policy.mode === "deny" ? "lockAll" : deny.length > 0 ? "manual" : "unlockAll";
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

// a weight is a whole number from 1: `create_route_target` refuses anything
// else, so the sheet says so before the save rather than after it
function weightValid(weight: string): boolean {
  return /^\d+$/.test(weight.trim()) && Number(weight) >= 1;
}

// an upstream model equal to the public name is the passthrough the backend
// stores as no model at all, so both are written — and compared — as absent
function upstreamFor(upstream: string | null | undefined, publicName: string): string | undefined {
  const u = (upstream ?? "").trim();
  return u && u !== publicName ? u : undefined;
}

function targetInput(target: DraftTarget, publicName: string) {
  return {
    provider_id: target.providerId,
    upstream_model: upstreamFor(target.upstream, publicName),
    weight: Number(target.weight),
  };
}

function sameTarget(
  row: RouteTargetRow,
  input: ReturnType<typeof targetInput>,
  publicName: string,
): boolean {
  return (
    row.provider_id === input.provider_id &&
    upstreamFor(row.upstream_model, publicName) === input.upstream_model &&
    row.weight === input.weight
  );
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
  providerName: (id: string) => string,
  advanced: Record<string, unknown>,
) {
  const { params, paramPolicy } = paramsToApi(draft);
  const publicName = draft.name.trim();
  const hasPricing = draft.price.input.trim() !== "" || draft.price.output.trim() !== "";
  const obj = {
    route: { model: publicName || undefined, strategy: draft.strategy, enabled: draft.enabled },
    targets: draft.targets.map((tg) => ({
      provider: providerName(tg.providerId) || undefined,
      upstream_model: upstreamFor(tg.upstream, publicName),
      weight: weightValid(tg.weight) ? Number(tg.weight) : tg.weight,
    })),
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
// the route's targets: provider, the model id sent to it, and a weight
// ---------------------------------------------------------------------------

function TargetEditor({
  targets,
  providers,
  publicName,
  strategy,
  labelId,
  emptyErrorId,
  weightErrorId,
  weightInvalid,
  onChange,
}: {
  targets: DraftTarget[];
  providers: ProviderRow[];
  publicName: string;
  strategy: string;
  /** the id the list is labelled by */
  labelId: string;
  /** the error saying a target is needed, which the add button points at */
  emptyErrorId: string;
  /** the error a weight field that fails validation points at */
  weightErrorId: string;
  weightInvalid: (target: DraftTarget) => boolean;
  onChange: (next: DraftTarget[]) => void;
}) {
  const { t } = useTranslation();
  const update = (i: number, patch: Partial<DraftTarget>) =>
    onChange(targets.map((tg, idx) => (idx === i ? { ...tg, ...patch } : tg)));
  // provider, then the upstream model and weight; below `sm` the provider takes
  // a line of its own so neither text field is squeezed to nothing
  const row =
    "grid grid-cols-[minmax(0,1fr)_72px_auto] items-center gap-2 sm:grid-cols-[minmax(0,1.1fr)_minmax(0,1.4fr)_72px_auto]";

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <FieldLabel
          label={t("modelSheet.targets.title")}
          required
          info={t("modelSheet.targets.info")}
          id={labelId}
        />
        <Button
          size="sm"
          variant="outline"
          disabled={providers.length === 0}
          aria-describedby={targets.length === 0 ? emptyErrorId : undefined}
          onClick={() => onChange([...targets, newTarget(providers[0]?.id ?? "")])}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("modelSheet.targets.add")}
        </Button>
      </div>
      {providers.length === 0 && (
        <p className="text-xs text-muted-foreground">{t("modelSheet.targets.noProviders")}</p>
      )}
      {targets.length > 0 && (
        // captions for the eye; every field names itself for a screen reader
        <div
          aria-hidden="true"
          className={cn(
            row,
            "text-[11px] uppercase tracking-[0.06em] text-[color:var(--text-subtle)] max-sm:hidden",
          )}
        >
          <span>{t("modelSheet.targets.provider")}</span>
          <span>{t("modelSheet.targets.upstream")}</span>
          <span>{t("modelSheet.targets.weight")}</span>
          <span className="w-8" />
        </div>
      )}
      <ul aria-labelledby={labelId} className="space-y-2">
        {targets.map((tg, i) => {
          const n = i + 1;
          const badWeight = weightInvalid(tg);
          return (
            <li key={tg.key} className={row}>
              <Combobox
                aria-label={t("modelSheet.targets.providerAria", { n })}
                className="col-span-3 font-mono sm:col-span-1"
                value={tg.providerId}
                onChange={(providerId) => update(i, { providerId })}
                options={providers.map((p) => ({ value: p.id, label: p.name }))}
              />
              <Input
                aria-label={t("modelSheet.targets.upstreamAria", { n })}
                className="font-mono"
                value={tg.upstream}
                placeholder={publicName || t("modelSheet.targets.upstreamPlaceholder")}
                onChange={(e) => update(i, { upstream: e.target.value })}
              />
              <Input
                aria-label={t("modelSheet.targets.weightAria", { n })}
                type="number"
                min={1}
                step={1}
                className="font-mono"
                value={tg.weight}
                aria-invalid={badWeight || undefined}
                aria-describedby={describedBy(badWeight && weightErrorId)}
                onChange={(e) => update(i, { weight: e.target.value })}
              />
              <DeleteIconButton
                label={t("modelSheet.targets.removeAria", { n })}
                title={t("common.remove")}
                onClick={() => onChange(targets.filter((_, idx) => idx !== i))}
              />
            </li>
          );
        })}
      </ul>
      {targets.length > 1 && !usesWeights(strategy) && (
        <p className="text-xs leading-snug text-muted-foreground">
          <Trans
            i18nKey="routeTargets.weightsIgnored"
            values={{ strategy }}
            components={[<span key="strategy" className="font-mono text-foreground" />]}
          />
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// the sheet
// ---------------------------------------------------------------------------

const SECTIONS = [
  "general",
  "routing",
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
  // view mode: that model's targets as the effective config states them, so
  // the read-only sheet shows where its traffic goes (#1979)
  configTargets?: RouteTargetView[] | null;
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
  configTargets,
  models,
  routes,
  onDone,
}: ModelSheetProps) {
  const queryClient = useQueryClient();
  const readonly = mode === "view";

  const [draft, setDraft] = React.useState<ModelDraft>(() => blankDraft(""));
  const [secOpen, setSecOpen] = React.useState<Record<SectionKey, boolean>>({
    general: true,
    routing: true,
    params: false,
    caps: false,
    pricing: false,
    advanced: false,
    headers: false,
    rbac: false,
    preview: false,
  });
  const [dupFrom, setDupFrom] = React.useState("");
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
      d.name = route.model;
      d.strategy = route.strategy;
      // every target the route has, not only the first: editing one line of a
      // fleet used to rewrite target 0 and never show the rest (#1979)
      d.targets = (targets.data ?? []).map((tg) =>
        newTarget(tg.provider_id, tg.upstream_model ?? "", String(tg.weight), tg.id),
      );
      d.enabled = route.enabled;
      seedAdvanced(d, route.advanced ?? {});
      seedParams(d, route.params ?? {}, route.param_policy ?? {});
      const price = prices.data?.find((p) => p.model === route.model);
      if (price) {
        d.price.input = price.input_per_mtok;
        d.price.output = price.output_per_mtok;
        d.price.cacheRead = price.cached_input_per_mtok ?? "";
        d.price.currency = price.currency || "USD";
      }
    } else if (mode === "view" && configModel) {
      d.name = configModel.model;
      d.strategy = configModel.strategy;
      d.targets = [];
    }
    setDraft(d);
    setDupFrom("");
    setSecOpen({
      general: true,
      routing: true,
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

  const dirty =
    !readonly && initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const { t } = useTranslation();
  const toast = useToast();

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

  const providerName = (id: string) => providers.find((p) => p.id === id)?.name ?? "";

  // -- validation (verbose, blocks save) ------------------------------------
  const publicName = draft.name.trim();
  const errName = !readonly && !publicName ? t("modelSheet.errors.name") : "";
  const nameConflict =
    !readonly &&
    publicName !== "" &&
    models.some(
      (m) =>
        m.model.toLowerCase() === publicName.toLowerCase() &&
        (mode !== "edit" || m.model !== route?.model),
    );
  const errNameTaken = nameConflict ? t("modelSheet.errors.nameTaken", { name: publicName }) : "";
  // a route with no target is accepted by the API and answers every request
  // with an error, so the sheet does not create one
  const errTargets = !readonly && draft.targets.length === 0 ? t("modelSheet.errors.targets") : "";
  const weightRowInvalid = (tg: DraftTarget) => !weightValid(tg.weight);
  const errWeight =
    !readonly && draft.targets.some(weightRowInvalid) ? t("modelSheet.errors.weight") : "";
  const errBaseUrl =
    draft.baseUrl.trim() !== "" && !/^https?:\/\//i.test(draft.baseUrl.trim())
      ? t("modelSheet.errors.baseUrl")
      : "";
  const paramRowInvalid = (p: (typeof draft.params)[number]) =>
    p.custom && p.value.trim() !== "" && p.key.trim() === "";
  const headerRowInvalid = (h: (typeof draft.headers)[number]) =>
    h.value.trim() !== "" && h.key.trim() === "";
  const errParam = draft.params.some(paramRowInvalid) ? t("modelSheet.errors.param") : "";
  const errHeader = draft.headers.some(headerRowInvalid) ? t("modelSheet.errors.header") : "";
  const errors = [
    errName,
    errNameTaken,
    errTargets,
    errWeight,
    errBaseUrl,
    errParam,
    errHeader,
  ].filter(Boolean);
  const canSave = !readonly && !editLoading && errors.length === 0;
  // the one the footer repeats beside the disabled button; the summary above it
  // still lists the rest
  const blockingError = readonly ? "" : (errors[0] ?? "");
  const blockingErrorId = React.useId();
  // one prefix for the ids tying each field to its hint and error
  const fid = React.useId();
  const ids = {
    nameHint: `${fid}-name-hint`,
    nameErr: `${fid}-name-err`,
    strategyHint: `${fid}-strategy-hint`,
    targetsLabel: `${fid}-targets-label`,
    targetsErr: `${fid}-targets-err`,
    weightErr: `${fid}-weight-err`,
    baseUrlHint: `${fid}-base-url-hint`,
    baseUrlErr: `${fid}-base-url-err`,
    paramErr: `${fid}-param-err`,
    headerErr: `${fid}-header-err`,
  };

  // -- persistence ----------------------------------------------------------
  // the route with its strategy, every target, default params with the lock
  // policy, the enabled flag and pricing go through their own endpoints; the
  // catalog metadata, limits, headers and visibility travel together as the
  // route's `advanced` blob (#1189).
  // form lifecycle for the UX stream (#805); names the form, never its contents
  const ux = useFormTelemetry(mode === "add" ? "model-create" : "model-edit", open, { dirty });
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
      const hasPricing = draft.price.input.trim() !== "" || draft.price.output.trim() !== "";
      if (mode === "add") {
        // the strategy the operator picked, not the first one on the list:
        // every model used to be created `round_robin` (#1979)
        const created = await createRoute(projectId as string, {
          model: publicName,
          strategy: draft.strategy,
        });
        // one at a time, so the targets are stored in the order they are listed
        for (const target of draft.targets) {
          await createRouteTarget(created.id, targetInput(target, publicName));
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
      // a target has no update endpoint (#2208), so a changed line is a new target and
      // the old one goes. every create runs before any delete: the gateway
      // picks up each write as it lands, and this order never leaves the route
      // with nothing to send to part-way through the save
      const stored = targets.data ?? [];
      const kept = new Set<string>();
      for (const target of draft.targets) {
        const input = targetInput(target, r.model);
        const was = target.id ? stored.find((row) => row.id === target.id) : undefined;
        if (was && sameTarget(was, input, r.model)) {
          kept.add(was.id);
          continue;
        }
        await createRouteTarget(r.id, input);
      }
      for (const row of stored) {
        if (!kept.has(row.id)) await deleteRouteTarget(row.id);
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
      // the catalog reads each route's targets from the effective config
      queryClient.invalidateQueries({ queryKey: ["config"] });
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

  // the sheet's own dismissal paths (Escape, scrim, close, Cancel) all run
  // through the shared discard prompt (#1463)
  const { guard, close, locked, prompt } = useDiscardGuard({
    dirty,
    saving: save.isPending,
    onOpenChange,
  });

  // duplicate-from: prefill the draft from an existing db route, then tweak.
  // the source's targets are read first, so the copy starts with the same
  // strategy and the same upstream models behind it
  const dupRequest = React.useRef("");
  const applyDupFrom = async (routeId: string) => {
    setDupFrom(routeId);
    dupRequest.current = routeId;
    if (!routeId) return;
    const src = routes.find((r) => r.id === routeId);
    if (!src) return;
    let srcTargets: RouteTargetRow[] = [];
    try {
      srcTargets = await queryClient.fetchQuery({
        queryKey: ["route-targets", src.id],
        queryFn: () => fetchRouteTargets(src.id),
      });
    } catch {
      // the name, strategy and params still copy; the operator adds targets
    }
    // a later pick won the race while this one was reading
    if (dupRequest.current !== routeId) return;
    setDraft((d) => {
      const next = blankDraft(d.targets[0]?.providerId || providers[0]?.id || "");
      next.name = src.model;
      next.strategy = src.strategy;
      next.enabled = src.enabled;
      // a passthrough target sent the source's name upstream; the copy is
      // renamed next, so it names that model outright instead
      if (srcTargets.length > 0) {
        next.targets = srcTargets.map((tg) =>
          newTarget(tg.provider_id, tg.upstream_model || src.model, String(tg.weight)),
        );
      }
      seedParams(next, src.params ?? {}, src.param_policy ?? {});
      return next;
    });
  };

  const title =
    mode === "add"
      ? t("modelSheet.titleAdd")
      : readonly
        ? t("modelSheet.titleView")
        : t("modelSheet.titleEdit");
  const subtitle =
    mode === "add" ? t("modelSheet.subtitleAdd") : `${publicName || "—"} · ${draft.strategy}`;
  const cta = mode === "add" ? t("modelSheet.ctaAdd") : t("modelSheet.ctaSave");

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
      ? t("modelSheet.lock.noteLockAll")
      : draft.paramMode === "unlockAll"
        ? t("modelSheet.lock.noteUnlockAll")
        : t("modelSheet.lock.noteManual");

  const lockModeOptions: { value: LockMode; label: string }[] = [
    { value: "lockAll", label: t("modelSheet.lock.lockAll") },
    { value: "unlockAll", label: t("modelSheet.lock.unlockAll") },
    { value: "manual", label: t("modelSheet.lock.manual") },
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
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SheetBody>
        {readonly && (
          <div className="flex items-start gap-2.5 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-3 py-2.5">
            <Lock className="mt-0.5 h-3.5 w-3.5 flex-none text-[color:var(--text-secondary)]" />
            <p className="text-xs leading-snug text-[color:var(--text-secondary)]">
              <Trans
                i18nKey="modelSheet.readonlyNotice"
                components={[<span key="code" className="font-mono text-foreground" />]}
              />
            </p>
          </div>
        )}
        {editLoading && <FormSkeleton fields={3} />}

        {mode === "add" && (
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.dupFrom.label")}
              info={t("modelSheet.dupFrom.info")}
              htmlFor="ms-field-1"
            />
            <Combobox
              id="ms-field-1"
              className="font-mono"
              value={dupFrom}
              onChange={applyDupFrom}
              options={[
                { value: "", label: t("modelSheet.dupFrom.scratch") },
                ...routes.map((r) => ({ value: r.id, label: r.model })),
              ]}
            />
          </div>
        )}

        {/* ===== General ===== */}
        <FormSection
          title={t("modelSheet.sections.general")}
          open={secOpen.general}
          onToggle={() => toggleSec("general")}
          className="space-y-3.5"
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
            <div className="space-y-1.5">
              <FieldLabel
                label={t("modelSheet.fields.name")}
                required
                info={t("modelSheet.fields.nameInfo")}
                htmlFor="ms-field-2"
              />
              <Input
                id="ms-field-2"
                className="font-mono"
                value={draft.name}
                placeholder="gpt-4o"
                disabled={readonly || mode === "edit"}
                aria-invalid={errName || errNameTaken ? true : undefined}
                aria-describedby={describedBy(
                  ids.nameHint,
                  (errName || errNameTaken) && ids.nameErr,
                )}
                onChange={(e) => set({ name: e.target.value })}
              />
              <p id={ids.nameHint} className="text-xs text-muted-foreground">
                {mode === "add"
                  ? t("modelSheet.fields.nameHint")
                  : t("modelSheet.fields.nameHintEdit")}
              </p>
              <FieldError id={ids.nameErr} error={errName || errNameTaken} />
            </div>
            <div className="space-y-1.5">
              <FieldLabel
                label={t("modelSheet.fields.modality")}
                info={t("modelSheet.fields.modalityInfo")}
                htmlFor="ms-field-3"
              />
              <Combobox
                id="ms-field-3"
                className="font-mono"
                value={draft.modality}
                disabled={readonly}
                onChange={(m) => setModality(m as Modality)}
                options={MODALITIES.map((m) => ({ value: m, label: m }))}
              />
            </div>
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.fields.baseUrl")}
              info={t("modelSheet.fields.baseUrlInfo")}
              htmlFor="ms-field-6"
            />
            <Input
              id="ms-field-6"
              className="font-mono"
              value={draft.baseUrl}
              placeholder="https://api.provider.com/v1"
              disabled={readonly}
              aria-invalid={errBaseUrl ? true : undefined}
              aria-describedby={describedBy(ids.baseUrlHint, errBaseUrl && ids.baseUrlErr)}
              onChange={(e) => set({ baseUrl: e.target.value })}
            />
            <p id={ids.baseUrlHint} className="text-xs text-muted-foreground">
              {t("modelSheet.fields.baseUrlHint")}
            </p>
            <FieldError id={ids.baseUrlErr} error={errBaseUrl} />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.fields.description")}
              info={t("modelSheet.fields.descriptionInfo")}
              htmlFor="ms-field-7"
            />
            <Textarea
              id="ms-field-7"
              value={draft.description}
              placeholder={t("modelSheet.fields.descriptionPlaceholder")}
              disabled={readonly}
              onChange={(e) => set({ description: e.target.value })}
            />
          </div>
          <SwitchRow
            title={t("modelSheet.fields.enabled")}
            hint={t("modelSheet.fields.enabledHint")}
            checked={draft.enabled}
            disabled={readonly}
            onChange={(v) => set({ enabled: v })}
          />
        </FormSection>

        {/* ===== Routing: how the route spreads traffic (#1979) ===== */}
        <FormSection
          title={t("modelSheet.sections.routing")}
          info={t("modelSheet.sections.routingInfo")}
          open={secOpen.routing}
          onToggle={() => toggleSec("routing")}
          className="space-y-4"
        >
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.routing.strategy")}
              info={t("modelSheet.routing.strategyInfo")}
              htmlFor="ms-strategy"
            />
            <Combobox
              id="ms-strategy"
              className="font-mono"
              value={draft.strategy}
              // the control plane takes a strategy when the route is created
              // and has no call that changes it afterwards (#2208)
              disabled={readonly || mode === "edit"}
              aria-describedby={mode === "edit" ? ids.strategyHint : undefined}
              onChange={(strategy) => set({ strategy })}
              options={strategyOptions(draft.strategy).map((s) => ({ value: s, label: s }))}
            />
            <StrategyHint strategy={draft.strategy} />
            {mode === "edit" && (
              <p id={ids.strategyHint} className="text-xs text-muted-foreground">
                {t("modelSheet.routing.strategyFixed")}
              </p>
            )}
          </div>
          {readonly ? (
            configTargets && configTargets.length > 0 ? (
              <div className="space-y-1.5">
                <p className="text-xs font-medium text-[color:var(--text-secondary)]">
                  {t("modelSheet.targets.title")}
                </p>
                <RouteTargetList
                  label={t("routeTargets.listLabel", { model: publicName })}
                  strategy={draft.strategy}
                  targets={configTargets}
                />
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">
                {t("routeTargets.count", { count: configModel?.targets ?? 0 })}
              </p>
            )
          ) : (
            <TargetEditor
              targets={draft.targets}
              providers={providers}
              publicName={publicName}
              strategy={draft.strategy}
              labelId={ids.targetsLabel}
              emptyErrorId={ids.targetsErr}
              weightErrorId={ids.weightErr}
              weightInvalid={weightRowInvalid}
              onChange={(next) => set({ targets: next })}
            />
          )}
          <FieldError id={ids.targetsErr} error={errTargets} />
          <FieldError id={ids.weightErr} error={errWeight} />
        </FormSection>

        {/* ===== Default parameters ===== */}
        <FormSection
          title={t("modelSheet.sections.params")}
          info={t("modelSheet.sections.paramsInfo")}
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
                    aria-label={t("modelSheet.params.name")}
                    className="h-[34px] flex-[1.1] font-mono text-xs"
                    value={p.key}
                    placeholder={t("modelSheet.params.namePlaceholder")}
                    disabled={readonly}
                    // the row the param error is about: a value with no name
                    aria-invalid={paramRowInvalid(p) || undefined}
                    aria-describedby={describedBy(paramRowInvalid(p) && ids.paramErr)}
                    onChange={(e) => setParamAt(i, { key: e.target.value })}
                  />
                ) : (
                  <span className="min-w-0 flex-[1.1] truncate font-mono text-sm">{p.key}</span>
                )}
                {p.type === "enum" ? (
                  <Combobox
                    aria-label={t("modelSheet.params.value")}
                    size="sm"
                    className="min-w-0 flex-1 font-mono"
                    value={p.value}
                    disabled={readonly}
                    onChange={(value) => setParamAt(i, { value })}
                    options={(p.opts ?? ["", "low", "medium", "high"]).map((o) => ({
                      value: o,
                      label: o === "" ? t("modelSheet.params.providerDefault") : o,
                    }))}
                  />
                ) : (
                  <Input
                    aria-label={t("modelSheet.params.value")}
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    type={p.type === "int" || p.type === "float" ? "number" : "text"}
                    step="any"
                    value={p.value}
                    placeholder={
                      p.custom
                        ? t("modelSheet.params.valuePlaceholder")
                        : t("modelSheet.params.providerDefault")
                    }
                    disabled={readonly}
                    onChange={(e) => setParamAt(i, { value: e.target.value })}
                  />
                )}
                {p.custom && (
                  <Combobox
                    aria-label={t("modelSheet.params.type")}
                    size="sm"
                    className="w-24 flex-none font-mono"
                    value={p.type}
                    disabled={readonly}
                    onChange={(type) => setParamAt(i, { type: type as ParamType })}
                    options={PARAM_TYPES.map((k) => ({ value: k, label: k }))}
                  />
                )}
                {paramManual && (
                  <LockButton
                    locked={p.locked}
                    disabled={readonly}
                    onToggle={() => setParamAt(i, { locked: !p.locked })}
                  />
                )}
                {p.custom && (
                  <DeleteIconButton
                    label={t("modelSheet.params.remove")}
                    title={t("common.remove")}
                    disabled={readonly}
                    onClick={() =>
                      setDraft((d) => ({
                        ...d,
                        params: d.params.filter((_, idx) => idx !== i),
                      }))
                    }
                  />
                )}
              </div>
            ))}
          </div>
          <FieldError id={ids.paramErr} error={errParam} />
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
              {t("modelSheet.params.add")}
            </Button>
          )}
        </FormSection>

        {/* ===== Capabilities (chat + audio only) ===== */}
        {showCaps && (
          <FormSection
            title={t("modelSheet.sections.caps")}
            info={t("modelSheet.sections.capsInfo")}
            open={secOpen.caps}
            onToggle={() => toggleSec("caps")}
            className="grid grid-cols-1 gap-2.5 sm:grid-cols-2"
          >
            <SwitchRow
              title={t("modelSheet.caps.streaming")}
              checked={draft.caps.streaming}
              disabled={readonly}
              onChange={(v) => setDeep("caps", { streaming: v })}
            />
            {draft.modality === "chat" && (
              <>
                <SwitchRow
                  title={t("modelSheet.caps.tools")}
                  checked={draft.caps.tools}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { tools: v })}
                />
                <SwitchRow
                  title={t("modelSheet.caps.vision")}
                  checked={draft.caps.vision}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { vision: v })}
                />
                <SwitchRow
                  title={t("modelSheet.caps.json")}
                  checked={draft.caps.json}
                  disabled={readonly}
                  onChange={(v) => setDeep("caps", { json: v })}
                />
                <SwitchRow
                  title={t("modelSheet.caps.reasoning")}
                  info={t("modelSheet.caps.reasoningInfo")}
                  checked={draft.caps.reasoning}
                  disabled={readonly}
                  onChange={setReasoning}
                />
              </>
            )}
          </FormSection>
        )}

        {/* ===== Pricing override ===== */}
        <FormSection
          title={t("modelSheet.sections.pricing")}
          info={t("modelSheet.sections.pricingInfo")}
          open={secOpen.pricing}
          onToggle={() => toggleSec("pricing")}
          className="space-y-3"
        >
          <p className="text-xs text-muted-foreground">{t("modelSheet.pricing.hint")}</p>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {(draft.modality === "chat" || draft.modality === "embedding") &&
              priceInput("input", t("modelSheet.pricing.input", { currency: cur }))}
            {draft.modality === "chat" && (
              <>
                {priceInput("output", t("modelSheet.pricing.output", { currency: cur }))}
                {priceInput("cacheWrite", t("modelSheet.pricing.cacheWrite", { currency: cur }))}
                {priceInput("cacheRead", t("modelSheet.pricing.cacheRead", { currency: cur }))}
              </>
            )}
          </div>
          {(draft.modality === "image" || draft.modality === "audio") && (
            <div className="space-y-1">
              <FieldLabel
                label={
                  draft.modality === "image"
                    ? t("modelSheet.pricing.perImage", { currency: cur })
                    : t("modelSheet.pricing.perMinute", { currency: cur })
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
              <FieldLabel label={t("modelSheet.pricing.currency")} htmlFor="ms-field-9" />
              <Combobox
                id="ms-field-9"
                className="font-mono"
                value={cur}
                disabled={readonly}
                onChange={(currency) => setDeep("price", { currency })}
                options={currencyOptions.map((c) => ({ value: c, label: c }))}
              />
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
        </FormSection>

        {/* ===== Limits & network ===== */}
        <FormSection
          title={t("modelSheet.sections.advanced")}
          open={secOpen.advanced}
          onToggle={() => toggleSec("advanced")}
          className="space-y-3"
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {numInput(
              "rpm",
              t("modelSheet.net.rpm"),
              t("modelSheet.net.unlimited"),
              t("modelSheet.net.rpmInfo"),
            )}
            {numInput(
              "tpm",
              t("modelSheet.net.tpm"),
              t("modelSheet.net.unlimited"),
              t("modelSheet.net.tpmInfo"),
            )}
            {numInput(
              "concurrency",
              t("modelSheet.net.concurrency"),
              t("modelSheet.net.unlimited"),
            )}
            {numInput("timeoutMs", t("modelSheet.net.timeout"), "30000")}
            {numInput("retries", t("modelSheet.net.retries"), "2")}
            {numInput("context", t("modelSheet.net.context"), "128000")}
            {numInput("maxOutput", t("modelSheet.net.maxOutput"), "16384")}
          </div>
          <SwitchRow
            title={t("modelSheet.net.insecureTls")}
            hint={t("modelSheet.net.insecureTlsHint")}
            info={t("modelSheet.net.insecureTlsInfo")}
            checked={draft.net.insecureTls}
            disabled={readonly}
            onChange={(v) => setDeep("net", { insecureTls: v })}
          />
        </FormSection>

        {/* ===== Custom request headers ===== */}
        <FormSection
          title={t("modelSheet.sections.headers")}
          info={t("modelSheet.sections.headersInfo")}
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
                    aria-label={t("modelSheet.headers.name")}
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    value={h.key}
                    placeholder={t("modelSheet.headers.namePlaceholder")}
                    disabled={readonly}
                    aria-invalid={headerRowInvalid(h) || undefined}
                    aria-describedby={describedBy(headerRowInvalid(h) && ids.headerErr)}
                    onChange={(e) => setHeaderAt(i, { key: e.target.value })}
                  />
                  <Input
                    aria-label={t("modelSheet.headers.value")}
                    className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                    value={h.value}
                    placeholder={t("modelSheet.headers.valuePlaceholder")}
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
                  <DeleteIconButton
                    label={t("modelSheet.headers.remove")}
                    title={t("common.remove")}
                    disabled={readonly}
                    onClick={() =>
                      setDraft((d) => ({
                        ...d,
                        headers: d.headers.filter((_, idx) => idx !== i),
                      }))
                    }
                  />
                </div>
              ))}
            </div>
          )}
          <FieldError id={ids.headerErr} error={errHeader} />
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
              {t("modelSheet.headers.add")}
            </Button>
          )}
        </FormSection>

        {/* ===== Access & permissions ===== */}
        <FormSection
          title={t("modelSheet.sections.rbac")}
          open={secOpen.rbac}
          onToggle={() => toggleSec("rbac")}
          className="space-y-3.5"
        >
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.rbac.minRole")}
              info={t("modelSheet.rbac.minRoleInfo")}
              htmlFor="ms-field-10"
            />
            <Combobox
              id="ms-field-10"
              className="font-mono"
              value={draft.rbac.minRole}
              disabled={readonly}
              onChange={(minRole) => setDeep("rbac", { minRole })}
              options={ROLES.map((r) => ({ value: r, label: r }))}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              label={t("modelSheet.rbac.visibility")}
              info={t("modelSheet.rbac.visibilityInfo")}
              id="ms-visibility-label"
            />
            <Segmented
              labelledBy="ms-visibility-label"
              value={draft.rbac.visibility}
              options={[
                { value: "public", label: t("modelSheet.rbac.public") },
                { value: "project", label: t("modelSheet.rbac.project") },
                { value: "restricted", label: t("modelSheet.rbac.restricted") },
              ]}
              disabled={readonly}
              onChange={(v) => setDeep("rbac", { visibility: v })}
            />
          </div>
          {draft.rbac.visibility === "restricted" && (
            <div className="space-y-3.5">
              <ChipGroup
                label={t("modelSheet.rbac.teams")}
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
                label={t("modelSheet.rbac.vkeys")}
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
                label={t("modelSheet.rbac.users")}
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
        </FormSection>

        {/* ===== Config preview ===== */}
        <FormSection
          title={t("modelSheet.configPreview")}
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
        </FormSection>
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
        <SheetError
          message={
            save.isError
              ? advancedRejected
                ? t("modelSheet.advancedRejected", { message: (save.error as Error).message })
                : (save.error as Error).message
              : undefined
          }
        />
        {/* no connection check here until the provider probe can say whether
            it serves this upstream model (#2008, #2009). the provider's own
            test answers "does the provider answer", which beside a model name
            reads as a claim about the model */}
        <SheetActions
          start={
            // the primary action stays where it is and greys out instead of
            // vanishing (#1265): a footer that reflows tells an operator who
            // never scrolled to the field errors only that saving is gone, so
            // the first error travels with the button and names the reason.
            // below `sm` the summary directly above already opens with that
            // same line, and a second copy squeezed beside the buttons is what
            // pushed Save off a phone (#2003), so there it is only announced.
            // a zero basis lets it give up width to the buttons rather than
            // wrap the row
            blockingError && (
              <p
                id={blockingErrorId}
                role="alert"
                className="text-xs leading-snug text-[color:var(--status-danger-text)] max-sm:sr-only sm:max-w-[52%] sm:flex-[1_1_0] sm:text-right"
              >
                {blockingError}
              </p>
            )
          }
        >
          <Button variant="ghost" disabled={locked} onClick={close}>
            {t("common.cancel")}
          </Button>
          {readonly && (
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.close")}
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
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
