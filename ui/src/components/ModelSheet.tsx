import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Lock, Plus, SearchCheck } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { FormSkeleton } from "@/components/LoadingState";
import { GatedButton } from "@/components/GatedButton";
import { PriceProvenanceLine } from "@/components/PriceProvenance";
import { ModelCheckOutcome } from "@/components/ProviderTestOutcome";
import { RouteTargetList } from "@/components/RouteTargetList";
import { StrategyPicker } from "@/components/StrategyPicker";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { DocsLink } from "@/components/DocsLink";
import { TakesOverField } from "@/components/TakesOverField";
import { ProviderLogo } from "@/components/ui/provider-logo";
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
  fetchConfig,
  fetchCurrencySettings,
  fetchModelPrices,
  fetchProviderModels,
  fetchRouteTargets,
  fetchRouteVariants,
  fetchTeams,
  fetchUsers,
  fetchVirtualKeys,
  isConvertible,
  ROLES,
  setRouteAdvanced,
  setRouteEnabled,
  STRATEGIES,
  testProvider,
  updateRoute,
  updateRouteParams,
  ApiError,
  type RouteTargetInput,
  type UpdateRouteInput,
  upsertModelPrice,
  type EffectiveModelDto,
  type ProviderRow,
  type RouteRow,
  type RouteTargetRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { readEffectivePolicy } from "@/lib/guardrail-policy";
import { keptRatePatches, type KeptRates } from "@/lib/cache-write-rate";
import { useErrorVisibility } from "@/lib/error-visibility";
import { ALL_TARGETS_RESTRICTED, isRestricted, onlyRestrictedDialled } from "@/lib/failure-classes";
import { providersUsableFrom } from "@/lib/provider-scope";
import type { RouteTargetView } from "@/lib/route-targets";
import { usesWeights } from "@/lib/strategies";
import { useToast } from "@/lib/toast";
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
export interface DraftTarget {
  key: string;
  id?: string;
  providerId: string;
  /** the model id sent upstream; blank sends the public name through as-is */
  upstream: string;
  weight: string;
  /**
   * the failure classes the target is a fallback for (#1584, #3319); empty
   * for an ordinary member of the pool
   */
  takesOver: string[];
}

let targetKeys = 0;
export function newTarget(
  providerId: string,
  upstream = "",
  weight = "1",
  id?: string,
  takesOver: string[] = [],
): DraftTarget {
  targetKeys += 1;
  return { key: `t${targetKeys}`, id, providerId, upstream, weight, takesOver };
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
  description: string;
  enabled: boolean;
  paramMode: LockMode;
  params: DraftParam[];
  caps: Caps;
  /** the model's context window in tokens, served on `GET /v1/models` and not enforced (#3265) */
  contextLength: string;
  price: {
    input: string;
    output: string;
    cacheWrite: string;
    cacheWrite1h: string;
    image: string;
    audioInput: string;
    speech: string;
    /** the audio token rates of a realtime or audio model (#3145); empty falls back to text */
    audioInputTokens: string;
    audioOutputTokens: string;
    cachedAudio: string;
    cacheRead: string;
    currency: string;
  };
  net: {
    timeoutMs: string;
    retries: string;
    maxOutput: string;
    /** requests per minute the route admits, across the fleet via Redis */
    rpm: string;
    /** tokens per minute the route admits, across the fleet via Redis */
    tpm: string;
    /** requests the route serves at once on each gateway replica */
    concurrency: string;
  };
  headerMode: LockMode;
  headers: DraftHeader[];
  /** the route's guardrail overrides, by rule name (`RouteGuardrails` in rolter-core) */
  guard: { disable: string[]; enable: string[] };
  rbac: {
    minRole: string;
    visibility: "public" | "project" | "restricted";
    teams: string[];
    vkeys: string[];
    users: string[];
  };
}

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

/**
 * The provider a new target line starts on: the only one there is, otherwise
 * none (#2810).
 *
 * A line used to start on the first provider alphabetically, so a route for
 * `llama-3.1-8b` saved without a second look sent its traffic to
 * `anthropic-direct`. With a choice to make, the operator makes it.
 */
export function defaultProviderId(providers: ProviderRow[]): string {
  return providers.length === 1 ? providers[0].id : "";
}

function blankDraft(
  providers: ProviderRow[],
  providerId = defaultProviderId(providers),
): ModelDraft {
  return {
    name: "",
    strategy: STRATEGIES[0],
    // one line to start from: the common case is one provider behind one
    // route, and a fleet adds lines from there. with no provider to offer there
    // is no line to fill in, and the note about adding one says why
    targets: providers.length > 0 ? [newTarget(providerId)] : [],
    modality: "chat",
    description: "",
    enabled: true,
    paramMode: "manual",
    params: paramDefs("chat", false),
    caps: defaultCaps("chat"),
    contextLength: "",
    price: {
      input: "",
      output: "",
      cacheWrite: "",
      cacheWrite1h: "",
      image: "",
      audioInput: "",
      speech: "",
      audioInputTokens: "",
      audioOutputTokens: "",
      cachedAudio: "",
      cacheRead: "",
      currency: "USD",
    },
    net: {
      timeoutMs: "",
      retries: "",
      maxOutput: "",
      rpm: "",
      tpm: "",
      concurrency: "",
    },
    headerMode: "manual",
    headers: [],
    guard: { disable: [], enable: [] },
    rbac: { minRole: "member", visibility: "public", teams: [], vkeys: [], users: [] },
  };
}

/**
 * Keys a route's `advanced` blob used to carry and the gateway never read
 * (#2924): a per-route endpoint and an insecure-TLS switch, and flat image and
 * audio prices. A save sheds them from the stored blob; the `context_window`
 * limit went the same way and drops out because the limits object is rebuilt
 * from the ones the gateway applies. `rpm`, `tpm` and `concurrency` left with
 * it and came back with a reader (#2936).
 */
const RETIRED_ADVANCED_KEYS = ["base_url", "insecure_tls", "pricing"] as const;

/** the most retries a route may ask for, which the control plane and gateway enforce */
const MAX_ROUTE_RETRIES = 10;

/** the largest `rpm` or `concurrency` the control plane accepts for a route (#2936) */
const MAX_ROUTE_THROUGHPUT = 10_000_000;
/** `tpm` is a u32 on the backend */
const MAX_ROUTE_TPM = 4_294_967_295;
/** `context_length` is a u32 on the backend (#3265) */
const MAX_CONTEXT_LENGTH = 4_294_967_295;

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
  draft.contextLength = num(advanced.context_length);
  draft.params = paramDefs(draft.modality, draft.caps.reasoning);
  draft.description = str(advanced.description);

  // the keys in `RETIRED_ADVANCED_KEYS` and the limits the gateway never read
  // (#2924) are not seeded: a field that is not on screen cannot be edited, and
  // the save sheds them from the stored blob
  const limits = obj(advanced.limits);
  // the backend stores whole seconds; the field is milliseconds
  draft.net.timeoutMs =
    typeof limits.timeout_secs === "number" ? String(limits.timeout_secs * 1000) : "";
  draft.net.retries = num(limits.retries);
  draft.net.maxOutput = num(limits.output_tokens);
  draft.net.rpm = num(limits.rpm);
  draft.net.tpm = num(limits.tpm);
  draft.net.concurrency = num(limits.concurrency);

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

  const guardrails = obj(advanced.guardrails);
  draft.guard = { disable: strings(guardrails.disable), enable: strings(guardrails.enable) };

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
  // "inherit the gateway setting", so neither is sent
  const limit = (v: string) => {
    const n = Math.trunc(Number(v));
    return v.trim() !== "" && Number.isFinite(n) && n > 0 ? n : undefined;
  };
  // the retry budget is the one limit where 0 is a value: it turns retries off
  // for the route, so only a blank field inherits the gateway's
  const retries = (v: string) => {
    const n = Math.trunc(Number(v));
    return v.trim() !== "" && Number.isFinite(n) && n >= 0 ? n : undefined;
  };
  // an absent field is absent, not null: every one is `Option`/`default` on the
  // backend and a null would fail to deserialize
  const put = (target: Record<string, unknown>, key: string, value: unknown) => {
    if (value === undefined) delete target[key];
    else target[key] = value;
  };

  const out = { ...stored };
  // keys the gateway never read, shed so the stored blob stops carrying a
  // setting that looks like policy (#2924). The control plane drops them on
  // write too; this keeps an older control plane from storing them again
  for (const key of RETIRED_ADVANCED_KEYS) delete out[key];
  out.model_type = draft.modality;
  out.capabilities = Object.entries(draft.caps)
    .filter(([, on]) => on)
    .map(([key]) => key);
  put(out, "context_length", limit(draft.contextLength));
  put(out, "description", draft.description.trim() || undefined);

  // rebuilt from the limits the gateway applies, so one it never read
  // (`context_window`) drops out of a stored blob on the next save
  const limits: Record<string, unknown> = {};
  put(limits, "retries", retries(draft.net.retries));
  put(limits, "output_tokens", limit(draft.net.maxOutput));
  put(limits, "rpm", limit(draft.net.rpm));
  put(limits, "tpm", limit(draft.net.tpm));
  put(limits, "concurrency", limit(draft.net.concurrency));
  // the field is milliseconds and the backend stores whole seconds; a
  // sub-second timeout rounds up to 1 rather than to the 0 it would refuse
  const timeoutMs = Number(draft.net.timeoutMs);
  if (draft.net.timeoutMs.trim() !== "" && Number.isFinite(timeoutMs) && timeoutMs > 0) {
    limits.timeout_secs = Math.max(1, Math.round(timeoutMs / 1000));
  }
  out.limits = limits;

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

  // the stored object is the base, so a key a newer control plane added to it
  // survives; an override that was emptied drops out rather than being sent as
  // `[]`, and an object left with nothing in it is not sent at all
  const guardrails = obj(stored.guardrails);
  put(guardrails, "disable", draft.guard.disable.length > 0 ? draft.guard.disable : undefined);
  put(guardrails, "enable", draft.guard.enable.length > 0 ? draft.guard.enable : undefined);
  put(out, "guardrails", Object.keys(guardrails).length > 0 ? guardrails : undefined);

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
export function weightValid(weight: string): boolean {
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
    takes_over: target.takesOver,
  };
}

export type RefusalField = "targets" | "provider" | "weight" | "takesOver";

/**
 * A refusal the control plane pinned to one part of the target list: the whole
 * list (`targets`), or one entry's field (`targets[1].weight`). The entry is
 * kept by its draft `key` rather than its index, so removing an earlier line
 * does not move the message onto a neighbour.
 */
export interface ServerRefusal {
  field: RefusalField;
  key?: string;
  /**
   * the list was refused because every target is a restricted fallback
   * (`all_targets_restricted`), which the sheet words apart from an empty list
   */
  allRestricted?: boolean;
}

/** `targets`, `targets[2].weight`, `targets[0].provider_id`, `targets[1].takes_over` */
export function refusalOf(error: unknown, targets: DraftTarget[]): ServerRefusal | null {
  if (!(error instanceof ApiError) || error.code !== "invalid_field" || !error.field) return null;
  const match = /^targets(?:\[(\d+)\]\.(weight|provider_id|takes_over))?$/.exec(error.field);
  if (!match) return null;
  if (match[1] === undefined) {
    return error.message.includes(ALL_TARGETS_RESTRICTED)
      ? { field: "targets", allRestricted: true }
      : { field: "targets" };
  }
  const key = targets[Number(match[1])]?.key;
  if (!key) return null;
  const field: RefusalField =
    match[2] === "weight" ? "weight" : match[2] === "takes_over" ? "takesOver" : "provider";
  return { field, key };
}

/**
 * The target list as `PUT /api/v1/routes/{id}` takes it.
 *
 * A stored line goes back with its `upstream_model` exactly as it was read, so
 * the entry matches the stored target by (provider, upstream model) and keeps
 * its id. That is why it does not use `upstreamFor`: a stored model that spells
 * out the route's own name would turn into "absent", a different key, and the
 * target would be replaced with a new id (#2235). A line added in the sheet has
 * no stored value to keep, so it is normalised like a created one.
 */
function targetsToApi(targets: DraftTarget[], publicName: string): RouteTargetInput[] {
  return targets.map((tg) => ({
    provider_id: tg.providerId,
    upstream_model: tg.id
      ? tg.upstream.trim() === ""
        ? null
        : tg.upstream
      : (upstreamFor(tg.upstream, publicName) ?? null),
    weight: Number(tg.weight),
    // always sent: an entry without it clears the stored set
    takes_over: tg.takesOver,
  }));
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
/** the kept rates the price row opened with, to tell an edit from an input left alone */
type SeededRates = KeptRates;

/**
 * what a save says about the rates the form does not always replace: the two
 * cache-write rates (#2876, #2902), the per-unit rates (#2937) and the audio
 * token rates (#3145). Only those that were edited are named, so an input left
 * alone keeps what is stored.
 */
function cacheWritePatches(draft: ModelDraft, seeded: SeededRates) {
  return keptRatePatches(draft.price, seeded);
}

/**
 * Whether the draft carries a price row to write: a token rate, or a per-unit
 * rate typed or already stored (a model billed by the image has no token rate).
 */
function pricesAnything(draft: ModelDraft) {
  const p = draft.price;
  return [
    p.input,
    p.output,
    p.image,
    p.audioInput,
    p.speech,
    p.audioInputTokens,
    p.audioOutputTokens,
    p.cachedAudio,
  ].some((v) => v.trim() !== "");
}

function buildPreview(
  draft: ModelDraft,
  providerName: (id: string) => string,
  advanced: Record<string, unknown>,
  seededRates: SeededRates,
  editing: boolean,
) {
  const { params, paramPolicy } = paramsToApi(draft);
  const publicName = draft.name.trim();
  const hasPricing = pricesAnything(draft);
  const obj = {
    route: { model: publicName || undefined, strategy: draft.strategy, enabled: draft.enabled },
    // an edit previews the list the update call will carry, stored models as stored
    targets: draft.targets.map((tg) => ({
      provider: providerName(tg.providerId) || undefined,
      upstream_model: editing
        ? targetsToApi([tg], publicName)[0].upstream_model
        : upstreamFor(tg.upstream, publicName),
      weight: weightValid(tg.weight) ? Number(tg.weight) : tg.weight,
      takes_over: tg.takesOver.length > 0 ? tg.takesOver : undefined,
    })),
    params,
    param_policy: paramPolicy,
    model_price: hasPricing
      ? {
          model: publicName || undefined,
          input_per_mtok: draft.price.input.trim() || "0",
          output_per_mtok: draft.price.output.trim() || "0",
          cached_input_per_mtok: draft.price.cacheRead.trim() || undefined,
          ...cacheWritePatches(draft, seededRates),
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

/** which of the target errors are on screen, as the sheet decided (#2810) */
export interface TargetErrorsShown {
  /** the route has no target at all */
  empty: boolean;
  /** a line has no provider */
  provider: boolean;
  /** a line's weight is not a whole number from 1 */
  weight: boolean;
}

export function TargetEditor({
  targets,
  providers,
  hidden,
  publicName,
  strategy,
  labelId,
  emptyErrorId,
  providerErrorId,
  weightErrorId,
  shown,
  providerInvalid,
  weightInvalid,
  takesOverError,
  onChange,
  onTouch,
}: {
  targets: DraftTarget[];
  providers: ProviderRow[];
  /** how many of the org's providers the route's project may not use, and so are not offered */
  hidden: number;
  publicName: string;
  strategy: string;
  /** the id the list is labelled by */
  labelId: string;
  /** the error saying a target is needed, which the add button points at */
  emptyErrorId: string;
  /** the error a line with no provider points at */
  providerErrorId: string;
  /** the error a weight field that fails validation points at */
  weightErrorId: string;
  shown: TargetErrorsShown;
  providerInvalid: (target: DraftTarget) => boolean;
  weightInvalid: (target: DraftTarget) => boolean;
  /** what is wrong with a target's `takes_over`, if anything; already translated */
  takesOverError?: (target: DraftTarget) => string | undefined;
  onChange: (next: DraftTarget[]) => void;
  /** a provider or weight field was visited, so its error may show */
  onTouch: (field: "provider" | "weight") => void;
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
          aria-describedby={shown.empty ? emptyErrorId : undefined}
          data-error-anchor={shown.empty || undefined}
          onClick={() => onChange([...targets, newTarget(defaultProviderId(providers))])}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("modelSheet.targets.add")}
        </Button>
      </div>
      {/* the provider a target needs cannot be made from inside this sheet,
          so the way out is a link to where it can (#1982) */}
      {providers.length === 0 && (
        <p className="text-xs text-muted-foreground">
          <Trans
            i18nKey="modelSheet.targets.noProviders"
            components={[
              <Link
                key="providers"
                to="/providers"
                className="text-foreground underline underline-offset-2 hover:no-underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              />,
            ]}
          />
        </p>
      )}
      {hidden > 0 && (
        <p className="text-xs text-muted-foreground">
          {t("modelSheet.targets.scopeNote", { count: hidden })}
        </p>
      )}
      {targets.length > 0 && (
        // captions for the eye; every field names itself for a screen reader
        <div
          aria-hidden="true"
          className={cn(
            row,
            "text-2xs uppercase tracking-[0.06em] text-[color:var(--text-subtle)] max-sm:hidden",
          )}
        >
          <span>{t("modelSheet.targets.provider")}</span>
          <span>{t("modelSheet.targets.upstream")}</span>
          <span>{t("modelSheet.targets.weight")}</span>
          <span className="w-8" />
        </div>
      )}
      <ul aria-labelledby={labelId} className="space-y-2">
        {targets.map((tg, i) => (
          <TargetRow
            key={tg.key}
            n={i + 1}
            target={tg}
            providers={providers}
            publicName={publicName}
            rowClass={row}
            badProvider={shown.provider && providerInvalid(tg)}
            badWeight={shown.weight && weightInvalid(tg)}
            providerErrorId={providerErrorId}
            weightErrorId={weightErrorId}
            takesOverError={takesOverError?.(tg)}
            onChange={(patch) => update(i, patch)}
            onRemove={() => onChange(targets.filter((_, idx) => idx !== i))}
            onTouch={onTouch}
          />
        ))}
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

/**
 * One target line: the provider, the model id sent to it and a weight.
 *
 * The model id is free text that suggests what the picked provider's upstream
 * lists (#2810), so an id is chosen rather than retyped to the letter. The list
 * is asked for when the operator reaches for it, not for every line the sheet
 * opens with, and an upstream that lists nothing leaves the field as the text
 * box it always was.
 */
function TargetRow({
  n,
  target,
  providers,
  publicName,
  rowClass,
  badProvider,
  badWeight,
  providerErrorId,
  weightErrorId,
  takesOverError,
  onChange,
  onRemove,
  onTouch,
}: {
  n: number;
  target: DraftTarget;
  providers: ProviderRow[];
  publicName: string;
  rowClass: string;
  badProvider: boolean;
  badWeight: boolean;
  providerErrorId: string;
  weightErrorId: string;
  takesOverError?: string;
  onChange: (patch: Partial<DraftTarget>) => void;
  onRemove: () => void;
  onTouch: (field: "provider" | "weight") => void;
}) {
  const { t } = useTranslation();
  const [asked, setAsked] = React.useState(false);
  const listed = useQuery({
    queryKey: ["provider-models", target.providerId],
    queryFn: () => fetchProviderModels(target.providerId),
    enabled: asked && target.providerId !== "",
    // the catalogue is an upstream call, and it rarely moves within a session
    staleTime: 5 * 60_000,
    retry: false,
  });
  const suggestions = React.useMemo(
    () => (listed.data?.models ?? []).map((id) => ({ value: id, label: id })),
    [listed.data],
  );
  // a refused or failed listing is not the provider having none: the control
  // plane answers an upstream that does not answer with an empty list, and
  // refuses a role that may not spend the provider's credential on it, so the
  // two read differently and neither stops the id being typed
  // what the provider says about the id this line would send: the name typed,
  // or the public name when the field is blank, since that is what goes
  // upstream. The answer belongs to that pair, so changing either clears it
  const checked = target.upstream.trim() || publicName.trim();
  const provider = providers.find((p) => p.id === target.providerId);
  const check = useMutation({
    mutationFn: () => testProvider(target.providerId, checked),
  });
  const resetCheck = check.reset;
  React.useEffect(() => {
    resetCheck();
  }, [target.providerId, checked, resetCheck]);
  const nothingToOffer = !target.providerId
    ? t("modelSheet.targets.upstreamNeedsProvider")
    : listed.isFetching
      ? t("modelSheet.targets.upstreamLoading")
      : listed.isError
        ? t("modelSheet.targets.upstreamUnavailable")
        : t("modelSheet.targets.upstreamNone");

  return (
    <li className={rowClass}>
      <Combobox
        aria-label={t("modelSheet.targets.providerAria", { n })}
        className="col-span-3 font-mono sm:col-span-1"
        value={target.providerId}
        placeholder={t("modelSheet.targets.providerPlaceholder")}
        aria-invalid={badProvider || undefined}
        aria-describedby={describedBy(badProvider && providerErrorId)}
        onChange={(providerId) => {
          onChange({ providerId });
          // the next thing they reach for is this provider's models
          setAsked(true);
        }}
        onBlur={() => onTouch("provider")}
        options={providers.map((p) => ({
          value: p.id,
          label: p.name,
          icon: <ProviderLogo kind={p.kind} name={p.name} />,
        }))}
      />
      <div className="flex min-w-0 items-center gap-1">
        <Combobox
          allowCustom
          clearable
          commitOnBlur
          aria-label={t("modelSheet.targets.upstreamAria", { n })}
          className="min-w-0 flex-1 font-mono"
          // model ids share long prefixes, so the list is wider than the field:
          // it grows leftward from the field's edge on the sheet, and fills the
          // line below it on a phone, where the field is a third of the width
          listClassName="sm:right-0 sm:w-96 max-sm:w-[calc(100vw-4.5rem)]"
          value={target.upstream}
          placeholder={publicName || t("modelSheet.targets.upstreamPlaceholder")}
          emptyText={nothingToOffer}
          options={suggestions}
          onChange={(upstream) => onChange({ upstream })}
          onFocus={() => setAsked(true)}
        />
        {/* the probe spends the provider's credential, so it is the provider
          editor's to run, as for the provider sheet's own test */}
        <GatedButton
          gate="provider:update"
          control="model-check-upstream"
          variant="outline"
          size="icon"
          className="shrink-0"
          disabled={!target.providerId || !checked || check.isPending}
          aria-label={t("modelSheet.targets.checkAria", { n })}
          title={t("modelSheet.targets.checkTitle")}
          onClick={() => check.mutate()}
        >
          {check.isPending ? (
            <Loader2 className="size-4 motion-safe:animate-spin" />
          ) : (
            <SearchCheck className="size-4" />
          )}
        </GatedButton>
      </div>
      <Input
        aria-label={t("modelSheet.targets.weightAria", { n })}
        type="number"
        min={1}
        step={1}
        className="font-mono"
        value={target.weight}
        aria-invalid={badWeight || undefined}
        aria-describedby={describedBy(badWeight && weightErrorId)}
        onChange={(e) => onChange({ weight: e.target.value })}
        onBlur={() => onTouch("weight")}
      />
      <DeleteIconButton
        label={t("modelSheet.targets.removeAria", { n })}
        title={t("common.remove")}
        onClick={onRemove}
      />
      {(check.isPending || check.data || check.isError) && (
        <div className="col-span-full" aria-live="polite">
          {check.isPending && (
            <p role="status" className="text-xs text-muted-foreground">
              {t("modelSheet.check.checking", { provider: provider?.name ?? "" })}
            </p>
          )}
          {check.data && (
            <ModelCheckOutcome
              result={check.data}
              model={checked}
              provider={provider?.name ?? ""}
            />
          )}
          {check.isError && (
            <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
              {errorText(check.error, t)}
            </p>
          )}
        </div>
      )}
      <TakesOverField
        groupLabel={t("fallbackTargets.groupAria", { n })}
        value={target.takesOver}
        error={takesOverError}
        onChange={(takesOver) => onChange({ takesOver })}
      />
    </li>
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
  "guardrails",
  "preview",
] as const;
type SectionKey = (typeof SECTIONS)[number];

/** the fields whose error the sheet can show, each one once */
type FieldKey =
  | "name"
  | "targets"
  | "provider"
  | "weight"
  | "retries"
  | "rpm"
  | "tpm"
  | "concurrency"
  | "contextLength"
  | "param"
  | "header"
  | "cacheWrite"
  | "cacheWrite1h"
  | "image"
  | "audioInput"
  | "speech"
  | "audioInputTokens"
  | "audioOutputTokens"
  | "cachedAudio"
  | "variants";

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
  providers: orgProviders,
  route,
  configModel,
  configTargets,
  models,
  routes,
  onDone,
}: ModelSheetProps) {
  const queryClient = useQueryClient();
  const readonly = mode === "view";
  // a route in project P may target P's own providers and org-wide ones, never
  // another project's, which the control plane refuses with a 409 (#1919)
  const routeProject = route?.project_id ?? projectId;
  const providers = React.useMemo(
    () => providersUsableFrom(orgProviders, routeProject),
    [orgProviders, routeProject],
  );

  const [draft, setDraft] = React.useState<ModelDraft>(() => blankDraft([]));
  // which errors are on screen: none on a pristine sheet, a field's own once it
  // was visited, all of them once a save was refused (#2810)
  const visibility = useErrorVisibility<FieldKey>();
  const [refusal, setRefusal] = React.useState<ServerRefusal | null>(null);
  const [secOpen, setSecOpen] = React.useState<Record<SectionKey, boolean>>({
    general: true,
    routing: true,
    params: false,
    caps: false,
    pricing: false,
    advanced: false,
    headers: false,
    rbac: false,
    guardrails: false,
    preview: false,
  });
  const [dupFrom, setDupFrom] = React.useState("");
  const initialRef = React.useRef("");
  // the price form as it was seeded, to tell a price left alone from one edited (#966)
  const initialPriceRef = React.useRef("");
  // the advanced payload as it was seeded, so a save can skip the extra PUT
  // when the operator changed nothing on that half of the form
  const initialAdvancedRef = React.useRef("");
  // the cache-write rates the price row opened with, so a save can tell an input
  // left alone (send nothing, the stored rate stays) from one that was emptied
  // (send null, which clears it) — see `cacheWritePatch` (#2876, #2902)
  const seededRatesRef = React.useRef<SeededRates>({
    cacheWrite: "",
    cacheWrite1h: "",
    image: "",
    audioInput: "",
    speech: "",
    audioInputTokens: "",
    audioOutputTokens: "",
    cachedAudio: "",
  });

  // data for edit-mode prefill
  const targets = useQuery({
    queryKey: ["route-targets", route?.id],
    queryFn: () => fetchRouteTargets(route!.id),
    enabled: open && mode === "edit" && !!route,
  });
  // the route's variants are not edited here, but their targets count with
  // the list's when the control plane decides a route is all fallbacks (#3373)
  const variants = useQuery({
    queryKey: ["route-variants", route?.id],
    queryFn: () => fetchRouteVariants(route!.id),
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

  // the rules an override can name are the effective policy's, and a config
  // route's own overrides are read out of the same document. any role may read
  // it, so a project admin gets the section too; best-effort like the chips
  const config = useQuery({
    queryKey: ["config"],
    queryFn: fetchConfig,
    enabled: open && mode !== "add",
    retry: false,
  });
  const policy = React.useMemo(() => readEffectivePolicy(config.data), [config.data]);

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
    const d = blankDraft(providers);
    if (mode === "edit" && route) {
      d.name = route.model;
      d.strategy = route.strategy;
      // every target the route has, not only the first: editing one line of a
      // fleet used to rewrite target 0 and never show the rest (#1979)
      d.targets = (targets.data ?? []).map((tg) =>
        newTarget(
          tg.provider_id,
          tg.upstream_model ?? "",
          String(tg.weight),
          tg.id,
          tg.takes_over ?? [],
        ),
      );
      d.enabled = route.enabled;
      seedAdvanced(d, route.advanced ?? {});
      seedParams(d, route.params ?? {}, route.param_policy ?? {});
      const price = prices.data?.find((p) => p.model === route.model);
      if (price) {
        d.price.input = price.input_per_mtok;
        d.price.output = price.output_per_mtok;
        d.price.cacheRead = price.cached_input_per_mtok ?? "";
        d.price.cacheWrite = price.cache_write_per_mtok ?? "";
        d.price.cacheWrite1h = price.cache_write_1h_per_mtok ?? "";
        d.price.image = price.image_per_unit ?? "";
        d.price.audioInput = price.audio_input_per_minute ?? "";
        d.price.speech = price.speech_per_mchar ?? "";
        d.price.audioInputTokens = price.audio_input_per_mtok ?? "";
        d.price.audioOutputTokens = price.audio_output_per_mtok ?? "";
        d.price.cachedAudio = price.cached_audio_input_per_mtok ?? "";
        d.price.currency = price.currency || "USD";
      }
    } else if (mode === "view" && configModel) {
      d.name = configModel.model;
      d.strategy = configModel.strategy;
      d.targets = [];
    }
    setDraft(d);
    seededRatesRef.current = {
      cacheWrite: d.price.cacheWrite,
      cacheWrite1h: d.price.cacheWrite1h,
      image: d.price.image,
      audioInput: d.price.audioInput,
      speech: d.price.speech,
      audioInputTokens: d.price.audioInputTokens,
      audioOutputTokens: d.price.audioOutputTokens,
      cachedAudio: d.price.cachedAudio,
    };
    visibility.reset();
    setRefusal(null);
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
      guardrails: false,
      preview: false,
    });
    initialRef.current = JSON.stringify(d);
    initialPriceRef.current = JSON.stringify(d.price);
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

  // the overrides a config-file route carries, which the sheet only shows.
  // the effective config also lists every organization's routes, and one may
  // share the file route's name (#1857), so only a route of the same owner is it
  const fileOverride =
    readonly && configModel
      ? policy?.routes.find(
          (route) =>
            route.model === configModel.model &&
            (route.org_id ?? "") === (configModel.org_id ?? ""),
        )
      : undefined;

  const providerName = (id: string) => providers.find((p) => p.id === id)?.name ?? "";

  // -- validation ------------------------------------------------------------
  // every error is computed on every render: the summary's count and the
  // refusal of a save read the same list. which of them are on screen is a
  // separate question, answered by `visibility` (#2810)
  const publicName = draft.name.trim();
  const nameMissing = !readonly && !publicName;
  // a name is taken only in this organization, across its projects, or by a
  // config-file model, which every organization shares: another
  // organization's model of the same name is no conflict (#1857, #3460), and
  // the control plane accepts it
  const nameConflict =
    !readonly &&
    publicName !== "" &&
    models.some(
      (m) =>
        (!m.org_id || m.org_id === orgId) &&
        m.model.toLowerCase() === publicName.toLowerCase() &&
        (mode !== "edit" || m.model !== route?.model),
    );
  const errName = nameMissing
    ? t("modelSheet.errors.name")
    : nameConflict
      ? t("modelSheet.errors.nameTaken", { name: publicName })
      : "";
  // a route with no target is accepted by the API and answers every request
  // with an error, so the sheet does not create one
  // a refusal from the control plane marks the same fields until the list is edited
  // a list of nothing but fallbacks could never dial anything, which the
  // control plane refuses as `all_targets_restricted` (#1584)
  // a route with stored variants dials only them, so the list does not count
  // then, as the control plane counts it (#3487); while they are unread the
  // list alone is judged, and the save's own refusal still says so
  const variantTargetSets = (Array.isArray(variants.data) ? variants.data : []).map((v) =>
    (v.targets ?? []).map((tg) => tg.takes_over ?? []),
  );
  const stranded =
    onlyRestrictedDialled(
      draft.targets.map((tg) => tg.takesOver),
      variantTargetSets,
    ) || !!refusal?.allRestricted;
  // with variants the list is not what strands the route, so the message sits
  // beside the variants instead; this sheet does not edit them, so it says
  // where they are edited (#3510)
  const variantsStranded = !readonly && stranded && variantTargetSets.length > 0;
  const onlyFallbacks = stranded && !variantsStranded;
  const errTargets = readonly
    ? ""
    : onlyFallbacks
      ? t("fallbackTargets.errors.allRestricted")
      : draft.targets.length === 0 || (refusal?.field === "targets" && !refusal.allRestricted)
        ? t("modelSheet.errors.targets")
        : "";
  const errVariants = variantsStranded
    ? t("fallbackTargets.errors.variantsAllRestrictedElsewhere")
    : "";
  // a set the control plane refused, said under that target's own set
  const takesOverError = (tg: DraftTarget) =>
    refusal?.field === "takesOver" && refusal.key === tg.key
      ? t("fallbackTargets.errors.refused")
      : undefined;
  // a target with no provider is not a target: the control plane would refuse
  // it, and a default that picked one for the operator sent traffic to the
  // wrong provider (#2810). only a blank line counts: a stored target whose
  // provider this project cannot use is kept as it is
  const providerRowInvalid = (tg: DraftTarget) => tg.providerId === "";
  const errProvider =
    !readonly && (draft.targets.some(providerRowInvalid) || refusal?.field === "provider")
      ? t("modelSheet.errors.provider")
      : "";
  const weightRowInvalid = (tg: DraftTarget) =>
    !weightValid(tg.weight) || (refusal?.field === "weight" && refusal.key === tg.key);
  const errWeight =
    !readonly && draft.targets.some(weightRowInvalid) ? t("modelSheet.errors.weight") : "";
  // blank inherits the gateway's budget; 0 turns retries off for the route
  const retriesTyped = draft.net.retries.trim();
  const errRetries =
    retriesTyped !== "" &&
    !(
      Number.isInteger(Number(retriesTyped)) &&
      Number(retriesTyped) >= 0 &&
      Number(retriesTyped) <= MAX_ROUTE_RETRIES
    )
      ? t("modelSheet.errors.retries", { max: MAX_ROUTE_RETRIES })
      : "";
  // blank means no cap; a cap is a whole number of at least 1, and the control
  // plane refuses 0 and anything above its ceiling (#2936)
  const capError = (typed: string, max: number, message: string) => {
    const value = typed.trim();
    if (readonly || value === "") return "";
    const n = Number(value);
    return Number.isInteger(n) && n >= 1 && n <= max ? "" : message;
  };
  const errRpm = capError(
    draft.net.rpm,
    MAX_ROUTE_THROUGHPUT,
    t("modelSheet.errors.rpm", { max: MAX_ROUTE_THROUGHPUT }),
  );
  const errTpm = capError(draft.net.tpm, MAX_ROUTE_TPM, t("modelSheet.errors.tpm"));
  const errConcurrency = capError(
    draft.net.concurrency,
    MAX_ROUTE_THROUGHPUT,
    t("modelSheet.errors.concurrency", { max: MAX_ROUTE_THROUGHPUT }),
  );
  const errContextLength = capError(
    draft.contextLength,
    MAX_CONTEXT_LENGTH,
    t("modelSheet.errors.contextLength"),
  );
  const paramRowInvalid = (p: (typeof draft.params)[number]) =>
    p.custom && p.value.trim() !== "" && p.key.trim() === "";
  const headerRowInvalid = (h: (typeof draft.headers)[number]) =>
    h.value.trim() !== "" && h.key.trim() === "";
  // the cache-write rates are saved with the price row, which is only written
  // once the input or output rate is set; a rate typed without either would be
  // dropped silently
  const hasPriceRow = draft.price.input.trim() !== "" || draft.price.output.trim() !== "";
  const writeRateError = (typed: string, notARate: string) => {
    if (readonly || draft.modality !== "chat" || typed.trim() === "") return "";
    const rate = Number(typed);
    if (!(Number.isFinite(rate) && rate >= 0)) return notARate;
    return hasPriceRow ? "" : t("modelSheet.errors.cacheWriteNeedsPrice");
  };
  const errCacheWrite = writeRateError(draft.price.cacheWrite, t("modelSheet.errors.cacheWrite"));
  const errCacheWrite1h = writeRateError(
    draft.price.cacheWrite1h,
    t("modelSheet.errors.cacheWrite1h"),
  );
  const unitRateError = (typed: string, modality: Modality) => {
    if (readonly || draft.modality !== modality || typed.trim() === "") return "";
    const rate = Number(typed);
    return Number.isFinite(rate) && rate >= 0 ? "" : t("modelSheet.errors.unitRate");
  };
  const errImage = unitRateError(draft.price.image, "image");
  const errAudioInput = unitRateError(draft.price.audioInput, "audio");
  const errSpeech = unitRateError(draft.price.speech, "audio");
  const errAudioInputTokens = unitRateError(draft.price.audioInputTokens, "audio");
  const errAudioOutputTokens = unitRateError(draft.price.audioOutputTokens, "audio");
  const errCachedAudio = unitRateError(draft.price.cachedAudio, "audio");
  const errParam = draft.params.some(paramRowInvalid) ? t("modelSheet.errors.param") : "";
  const errHeader = draft.headers.some(headerRowInvalid) ? t("modelSheet.errors.header") : "";
  const found: Record<FieldKey, string> = {
    name: errName,
    targets: errTargets,
    provider: errProvider,
    weight: errWeight,
    retries: errRetries,
    rpm: errRpm,
    tpm: errTpm,
    concurrency: errConcurrency,
    contextLength: errContextLength,
    param: errParam,
    header: errHeader,
    cacheWrite: errCacheWrite,
    cacheWrite1h: errCacheWrite1h,
    image: errImage,
    audioInput: errAudioInput,
    speech: errSpeech,
    audioInputTokens: errAudioInputTokens,
    audioOutputTokens: errAudioOutputTokens,
    cachedAudio: errCachedAudio,
    variants: errVariants,
  };
  const problems = (Object.keys(found) as FieldKey[]).filter((key) => found[key]);
  // what the sheet says under each field: its error, once it may be shown
  // a refusal from the control plane is already news: show it without waiting
  // for the field to be visited
  const shownError = (key: FieldKey) =>
    visibility.shows(key) || refusal?.field === key ? found[key] : "";
  // the list's refusal for holding only fallbacks is the variants' news when
  // the route has them
  const variantsMessage =
    visibility.shows("variants") || refusal?.allRestricted ? found.variants : "";
  const rowNumber = (key?: string) => draft.targets.findIndex((tg) => tg.key === key) + 1;
  const weightMessage =
    refusal?.field === "weight" && !draft.targets.some((tg) => !weightValid(tg.weight))
      ? t("routeSheet.errors.weightRow", { n: rowNumber(refusal.key) })
      : shownError("weight");
  // a refused save puts one line above the buttons, which counts what is left
  // to fix; each problem itself is stated once, at its own field
  const summary = visibility.attempts > 0 && problems.length > 0;
  // one prefix for the ids tying each field to its hint and error
  const fid = React.useId();
  const ids = {
    nameHint: `${fid}-name-hint`,
    nameErr: `${fid}-name-err`,
    targetsLabel: `${fid}-targets-label`,
    targetsErr: `${fid}-targets-err`,
    providerErr: `${fid}-provider-err`,
    weightErr: `${fid}-weight-err`,
    variantsLabel: `${fid}-variants-label`,
    variantsErr: `${fid}-variants-err`,
    timeoutHint: `${fid}-timeout-hint`,
    retriesHint: `${fid}-retries-hint`,
    retriesErr: `${fid}-retries-err`,
    maxOutputHint: `${fid}-max-output-hint`,
    rpmHint: `${fid}-rpm-hint`,
    rpmErr: `${fid}-rpm-err`,
    tpmHint: `${fid}-tpm-hint`,
    tpmErr: `${fid}-tpm-err`,
    concurrencyHint: `${fid}-concurrency-hint`,
    concurrencyErr: `${fid}-concurrency-err`,
    contextLengthHint: `${fid}-context-length-hint`,
    contextLengthErr: `${fid}-context-length-err`,
    paramErr: `${fid}-param-err`,
    headerErr: `${fid}-header-err`,
    cacheWriteHint: `${fid}-cache-write-hint`,
    cacheWriteErr: `${fid}-cache-write-err`,
    cacheWrite1hHint: `${fid}-cache-write-1h-hint`,
    cacheWrite1hErr: `${fid}-cache-write-1h-err`,
    imageHint: `${fid}-image-hint`,
    imageErr: `${fid}-image-err`,
    audioInputHint: `${fid}-audio-input-hint`,
    audioInputErr: `${fid}-audio-input-err`,
    speechHint: `${fid}-speech-hint`,
    speechErr: `${fid}-speech-err`,
    audioInputTokensHint: `${fid}-audio-input-tokens-hint`,
    audioInputTokensErr: `${fid}-audio-input-tokens-err`,
    audioOutputTokensHint: `${fid}-audio-output-tokens-hint`,
    audioOutputTokensErr: `${fid}-audio-output-tokens-err`,
    cachedAudioHint: `${fid}-cached-audio-hint`,
    cachedAudioErr: `${fid}-cached-audio-err`,
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
      const hasPricing = pricesAnything(draft);
      if (mode === "add") {
        // the strategy the operator picked, not the first one on the list:
        // every model used to be created `round_robin` (#1979)
        const created = await createRoute(projectId as string, {
          model: publicName,
          strategy: draft.strategy,
        });
        // one at a time, so the targets are stored in the order they are
        // listed, the ordinary ones first: a fallback added to a route with no
        // ordinary target yet is refused (`all_targets_restricted`), and a
        // fallback is never dialled first, so this moves no traffic
        const ordered = [
          ...draft.targets.filter((tg) => !isRestricted(tg.takesOver)),
          ...draft.targets.filter((tg) => isRestricted(tg.takesOver)),
        ];
        for (const target of ordered) {
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
            ...cacheWritePatches(draft, seededRatesRef.current),
            currency: draft.price.currency,
          });
        }
        await writeAdvanced(created.id);
        return;
      }
      // edit
      const r = route!;
      await updateRouteParams(r.id, params, paramPolicy);
      // one request for the enabled flag, the strategy and the whole target
      // list: the control plane applies them in a single transaction, so a
      // refused save leaves the route as it was, and a weight edit keeps the
      // target's id
      const input: UpdateRouteInput = { targets: targetsToApi(draft.targets, r.model) };
      if (draft.enabled !== r.enabled) input.enabled = draft.enabled;
      if (draft.strategy !== r.strategy) input.strategy = draft.strategy;
      await updateRoute(r.id, input);
      // a save writes the price with the same PUT an operator uses, which makes
      // it an override. so a catalogue price the form never touched is not
      // written back: saving the route must not turn it into one (#966)
      const catalogueUntouched =
        !!prices.data?.find((p) => p.model === r.model)?.source &&
        initialPriceRef.current === JSON.stringify(draft.price);
      if (hasPricing && !catalogueUntouched) {
        await upsertModelPrice({
          model: r.model,
          input_per_mtok: draft.price.input.trim() || "0",
          output_per_mtok: draft.price.output.trim() || "0",
          cached_input_per_mtok: draft.price.cacheRead.trim() || undefined,
          ...cacheWritePatches(draft, seededRatesRef.current),
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
      // a refusal about one line is said at that line; anything else has no row to sit on
      const named = mode === "edit" ? refusalOf(error, draft.targets) : null;
      if (named) {
        setRefusal(named);
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: publicName }),
        detail: errorText(error, t),
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

  // a save with something to fix does not go out: it shows every error, opens
  // the sections that hold one, and the effect below moves focus to the first
  const bodyRef = React.useRef<HTMLDivElement>(null);
  const requestSave = () => {
    if (problems.length > 0) {
      visibility.attempt();
      setSecOpen((open) => ({
        ...open,
        params: open.params || problems.includes("param"),
        headers: open.headers || problems.includes("header"),
        pricing:
          open.pricing ||
          problems.some((p) =>
            [
              "cacheWrite",
              "cacheWrite1h",
              "image",
              "audioInput",
              "speech",
              "audioInputTokens",
              "audioOutputTokens",
              "cachedAudio",
            ].includes(p),
          ),
      }));
      return;
    }
    ux.submitted();
    save.mutate();
  };
  const attempts = visibility.attempts;
  React.useEffect(() => {
    if (attempts === 0) return;
    const first = bodyRef.current?.querySelector<HTMLElement>(
      '[aria-invalid="true"], [data-error-anchor="true"]',
    );
    first?.focus();
  }, [attempts]);

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
      const next = blankDraft(providers, d.targets[0]?.providerId || defaultProviderId(providers));
      next.name = src.model;
      next.strategy = src.strategy;
      next.enabled = src.enabled;
      // a passthrough target sent the source's name upstream; the copy is
      // renamed next, so it names that model outright instead
      if (srcTargets.length > 0) {
        next.targets = srcTargets.map((tg) =>
          newTarget(
            tg.provider_id,
            tg.upstream_model || src.model,
            String(tg.weight),
            undefined,
            tg.takes_over ?? [],
          ),
        );
      }
      seedParams(next, src.params ?? {}, src.param_policy ?? {});
      return next;
    });
    // the copy carries a name that is taken until it is renamed, and nobody
    // typed it: saying so now is the reminder to rename, not a premature error
    visibility.touch("name");
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
  // cost is computed from token counts, so a model type billed per image or per
  // minute has no rate to set here (#2924)
  const tokenPriced = draft.modality === "chat" || draft.modality === "embedding";
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

  // a limit the gateway applies, with what blank means said under it. `check`
  // names the field whose error this input shows, when it has one
  const numInput = (
    key: keyof ModelDraft["net"],
    label: string,
    placeholder: string,
    hint: { id: string; text: string },
    check?: { field: FieldKey; errId: string },
  ) => {
    const error = check ? shownError(check.field) : "";
    return (
      <div className="space-y-1">
        <FieldLabel label={label} htmlFor={`ms-net-${key}`} />
        <Input
          id={`ms-net-${key}`}
          type="number"
          className="font-mono"
          value={draft.net[key]}
          placeholder={placeholder}
          disabled={readonly}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy(hint.id, error && check?.errId)}
          onChange={(e) => setDeep("net", { [key]: e.target.value } as Partial<ModelDraft["net"]>)}
          onBlur={check ? () => visibility.touch(check.field) : undefined}
        />
        <p id={hint.id} className="text-xs text-muted-foreground">
          {hint.text}
        </p>
        {check && <FieldError id={check.errId} error={error} />}
      </div>
    );
  };

  const priceInput = (
    key:
      | "input"
      | "output"
      | "cacheWrite"
      | "cacheWrite1h"
      | "image"
      | "audioInput"
      | "speech"
      | "audioInputTokens"
      | "audioOutputTokens"
      | "cachedAudio"
      | "cacheRead",
    label: string,
  ) => {
    // the cache-write rates are the price inputs that may stay empty on a priced
    // model, and say what empty means, so each carries a hint and an error
    const write =
      key === "cacheWrite"
        ? {
            field: "cacheWrite" as const,
            placeholder: t("modelSheet.pricing.cacheWritePlaceholder"),
            hint: t("modelSheet.pricing.cacheWriteHint"),
            hintId: ids.cacheWriteHint,
            errId: ids.cacheWriteErr,
          }
        : key === "cacheWrite1h"
          ? {
              field: "cacheWrite1h" as const,
              placeholder: t("modelSheet.pricing.cacheWrite1hPlaceholder"),
              hint: t("modelSheet.pricing.cacheWrite1hHint"),
              hintId: ids.cacheWrite1hHint,
              errId: ids.cacheWrite1hErr,
            }
          : key === "image" || key === "audioInput" || key === "speech"
            ? {
                field: key,
                placeholder: t("modelSheet.pricing.unitPlaceholder"),
                hint: t(`modelSheet.pricing.${key}Hint`),
                hintId: ids[`${key}Hint`],
                errId: ids[`${key}Err`],
              }
            : key === "audioInputTokens" || key === "audioOutputTokens" || key === "cachedAudio"
              ? {
                  // empty is not "not billed" here: it falls back to the text rate
                  field: key,
                  placeholder: t(`modelSheet.pricing.${key}Placeholder`),
                  hint: t(`modelSheet.pricing.${key}Hint`),
                  hintId: ids[`${key}Hint`],
                  errId: ids[`${key}Err`],
                }
              : null;
    const writeError = write ? shownError(write.field) : "";
    return (
      <div className="space-y-1">
        <FieldLabel label={label} htmlFor={`ms-price-${key}`} />
        <Input
          id={`ms-price-${key}`}
          type="number"
          step="any"
          className="font-mono"
          value={draft.price[key]}
          placeholder={write?.placeholder ?? "0.00"}
          disabled={readonly}
          aria-invalid={writeError ? true : undefined}
          aria-describedby={
            write ? describedBy(write.hintId, writeError && write.errId) : undefined
          }
          onChange={(e) => setDeep("price", { [key]: e.target.value })}
          onBlur={write ? () => visibility.touch(write.field) : undefined}
        />
        {write && (
          <>
            <p id={write.hintId} className="text-xs text-muted-foreground">
              {write.hint}
            </p>
            <FieldError id={write.errId} error={writeError} />
          </>
        )}
      </div>
    );
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SheetBody ref={bodyRef}>
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
                aria-invalid={shownError("name") ? true : undefined}
                aria-describedby={describedBy(ids.nameHint, shownError("name") && ids.nameErr)}
                onChange={(e) => set({ name: e.target.value })}
                onBlur={() => visibility.touch("name")}
              />
              <p id={ids.nameHint} className="text-xs text-muted-foreground">
                {mode === "add"
                  ? t("modelSheet.fields.nameHint")
                  : t("modelSheet.fields.nameHintEdit")}
              </p>
              <FieldError id={ids.nameErr} error={shownError("name")} />
            </div>
            {!readonly && (
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
            )}
          </div>
          {readonly && configModel && (
            <p className="text-xs text-muted-foreground">
              {configModel.source === "config"
                ? t("modelSheet.view.sourceConfig")
                : t("modelSheet.view.sourceDb")}
            </p>
          )}
          {!readonly && (
            <>
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
            </>
          )}
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
            <StrategyPicker
              id="ms-strategy"
              value={draft.strategy}
              disabled={readonly}
              onChange={(strategy) => set({ strategy })}
            />
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
              hidden={orgProviders.length - providers.length}
              publicName={publicName}
              strategy={draft.strategy}
              labelId={ids.targetsLabel}
              emptyErrorId={ids.targetsErr}
              providerErrorId={ids.providerErr}
              weightErrorId={ids.weightErr}
              shown={{
                empty: !!shownError("targets"),
                provider: !!shownError("provider"),
                weight: !!weightMessage,
              }}
              providerInvalid={providerRowInvalid}
              weightInvalid={weightRowInvalid}
              takesOverError={takesOverError}
              onChange={(next) => {
                set({ targets: next });
                setRefusal(null);
                // emptying the list is what the "add a target" error is about
                if (next.length < draft.targets.length) visibility.touch("targets");
              }}
              onTouch={visibility.touch}
            />
          )}
          <FieldError id={ids.targetsErr} error={shownError("targets")} />
          <FieldError id={ids.providerErr} error={shownError("provider")} />
          <FieldError id={ids.weightErr} error={weightMessage} />
          {variantsMessage && (
            <div className="space-y-1.5">
              <FieldLabel label={t("routeSheet.variants.title")} id={ids.variantsLabel} />
              <FieldError id={ids.variantsErr} error={variantsMessage} />
              {/* the variants are edited in the route sheet, so the way there
                  is what takes focus when the save is refused for them */}
              <Link
                to="/routing-rules"
                data-error-anchor="true"
                aria-describedby={describedBy(ids.variantsLabel, ids.variantsErr)}
                className="text-xs text-foreground underline underline-offset-2 hover:no-underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t("modelSheet.routing.editVariants")}
              </Link>
            </div>
          )}
        </FormSection>

        {/* a config route's parameters, caps, prices, headers and access are
            not in what /api/v1/models carries, so view mode shows none of them
            rather than a disabled form of defaults posing as its config (#1982) */}
        {!readonly && (
          <>
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
                        aria-invalid={(!!shownError("param") && paramRowInvalid(p)) || undefined}
                        aria-describedby={describedBy(
                          !!shownError("param") && paramRowInvalid(p) && ids.paramErr,
                        )}
                        onChange={(e) => setParamAt(i, { key: e.target.value })}
                        onBlur={() => visibility.touch("param")}
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
                        onBlur={() => visibility.touch("param")}
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
              <FieldError id={ids.paramErr} error={shownError("param")} />
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
              {/* an imported price is a catalogue value that can change; one an
                  operator set is theirs (#966). nothing for a model with no price */}
              <PriceProvenanceLine
                price={
                  mode === "edit" ? prices.data?.find((p) => p.model === route?.model) : undefined
                }
              />
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                {tokenPriced &&
                  priceInput("input", t("modelSheet.pricing.input", { currency: cur }))}
                {draft.modality === "chat" && (
                  <>
                    {priceInput("output", t("modelSheet.pricing.output", { currency: cur }))}
                    {priceInput(
                      "cacheWrite",
                      t("modelSheet.pricing.cacheWrite", { currency: cur }),
                    )}
                    {priceInput(
                      "cacheWrite1h",
                      t("modelSheet.pricing.cacheWrite1h", { currency: cur }),
                    )}
                    {priceInput("cacheRead", t("modelSheet.pricing.cacheRead", { currency: cur }))}
                  </>
                )}
                {draft.modality === "image" &&
                  priceInput("image", t("modelSheet.pricing.image", { currency: cur }))}
                {draft.modality === "audio" && (
                  <>
                    {priceInput(
                      "audioInput",
                      t("modelSheet.pricing.audioInput", { currency: cur }),
                    )}
                    {priceInput("speech", t("modelSheet.pricing.speech", { currency: cur }))}
                    {priceInput(
                      "audioInputTokens",
                      t("modelSheet.pricing.audioInputTokens", { currency: cur }),
                    )}
                    {priceInput(
                      "audioOutputTokens",
                      t("modelSheet.pricing.audioOutputTokens", { currency: cur }),
                    )}
                    {priceInput(
                      "cachedAudio",
                      t("modelSheet.pricing.cachedAudio", { currency: cur }),
                    )}
                  </>
                )}
              </div>
              <div className="flex items-end gap-3">
                {tokenPriced && (
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
                )}
                {/* rolter ships no pricing catalog, so the link explains how a
                request's cost is computed from the prices typed here (#977). it
                goes through the configured docs host and renders nothing on a
                deployment without one (#1981) */}
                <DocsLink
                  page="costTracking"
                  label={t("docs.link.costTracking")}
                  className="pb-2 text-xs"
                />
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
                {numInput("timeoutMs", t("modelSheet.net.timeout"), "60000", {
                  id: ids.timeoutHint,
                  text: t("modelSheet.net.timeoutHint"),
                })}
                {numInput(
                  "retries",
                  t("modelSheet.net.retries"),
                  "2",
                  { id: ids.retriesHint, text: t("modelSheet.net.retriesHint") },
                  { field: "retries", errId: ids.retriesErr },
                )}
                {numInput("maxOutput", t("modelSheet.net.maxOutput"), "16384", {
                  id: ids.maxOutputHint,
                  text: t("modelSheet.net.maxOutputHint"),
                })}
                {numInput(
                  "rpm",
                  t("modelSheet.net.rpm"),
                  "600",
                  { id: ids.rpmHint, text: t("modelSheet.net.rpmHint") },
                  { field: "rpm", errId: ids.rpmErr },
                )}
                {numInput(
                  "tpm",
                  t("modelSheet.net.tpm"),
                  "200000",
                  { id: ids.tpmHint, text: t("modelSheet.net.tpmHint") },
                  { field: "tpm", errId: ids.tpmErr },
                )}
                {numInput(
                  "concurrency",
                  t("modelSheet.net.concurrency"),
                  "8",
                  { id: ids.concurrencyHint, text: t("modelSheet.net.concurrencyHint") },
                  { field: "concurrency", errId: ids.concurrencyErr },
                )}
                {/* served on /v1/models whatever the model type, so it sits in
                    a section every type shows rather than with the chat and
                    audio flags (#3406) */}
                <div className="space-y-1 sm:col-span-2">
                  <FieldLabel
                    label={t("modelSheet.caps.contextLength")}
                    htmlFor="ms-context-length"
                  />
                  <Input
                    id="ms-context-length"
                    type="number"
                    min={1}
                    className="font-mono"
                    value={draft.contextLength}
                    placeholder="128000"
                    disabled={readonly}
                    aria-invalid={shownError("contextLength") ? true : undefined}
                    aria-describedby={describedBy(
                      ids.contextLengthHint,
                      shownError("contextLength") && ids.contextLengthErr,
                    )}
                    onChange={(e) => set({ contextLength: e.target.value })}
                    onBlur={() => visibility.touch("contextLength")}
                  />
                  <p id={ids.contextLengthHint} className="text-xs text-muted-foreground">
                    {t("modelSheet.caps.contextLengthHint")}
                  </p>
                  <FieldError id={ids.contextLengthErr} error={shownError("contextLength")} />
                </div>
              </div>
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
                        aria-invalid={(!!shownError("header") && headerRowInvalid(h)) || undefined}
                        aria-describedby={describedBy(
                          !!shownError("header") && headerRowInvalid(h) && ids.headerErr,
                        )}
                        onChange={(e) => setHeaderAt(i, { key: e.target.value })}
                        onBlur={() => visibility.touch("header")}
                      />
                      <Input
                        aria-label={t("modelSheet.headers.value")}
                        className="h-[34px] min-w-0 flex-1 font-mono text-xs"
                        value={h.value}
                        placeholder={t("modelSheet.headers.valuePlaceholder")}
                        disabled={readonly}
                        onChange={(e) => setHeaderAt(i, { value: e.target.value })}
                        onBlur={() => visibility.touch("header")}
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
              <FieldError id={ids.headerErr} error={shownError("header")} />
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
          </>
        )}

        {/* ===== Guardrail overrides ===== */}
        <FormSection
          title={t("modelSheet.sections.guardrails")}
          info={t("modelSheet.sections.guardrailsInfo")}
          open={secOpen.guardrails}
          onToggle={() => toggleSec("guardrails")}
          className="space-y-3.5"
        >
          {readonly && (
            <p className="text-xs text-muted-foreground">{t("modelSheet.guardrails.fileRoute")}</p>
          )}
          {!readonly && policy === null && mode === "edit" && !config.isLoading && (
            <p className="text-xs text-muted-foreground">{t("modelSheet.guardrails.unreadable")}</p>
          )}
          {!readonly && policy !== null && policy.rules.length === 0 && (
            <p className="text-xs text-muted-foreground">{t("modelSheet.guardrails.noRules")}</p>
          )}
          {(["disable", "enable"] as const).map((list) => {
            const names = new Set(policy?.rules.map((rule) => rule.name.trim()));
            const chosen = readonly ? (fileOverride?.[list] ?? []) : draft.guard[list];
            // a stored name that matches no rule stays a chip, marked, so it can
            // be removed; dropping it silently would hide a typo that the
            // gateway refuses at load
            const stale = policy ? chosen.filter((name) => !names.has(name)) : chosen;
            return (
              <ChipGroup
                key={list}
                label={t(`modelSheet.guardrails.${list}`)}
                options={[
                  ...(policy?.rules ?? []).map((rule) => ({
                    id: rule.name.trim(),
                    name: rule.name.trim(),
                  })),
                  ...stale.map((name) => ({
                    id: name,
                    name: policy ? t("modelSheet.guardrails.unknownRule", { name }) : name,
                  })),
                ]}
                selected={chosen}
                disabled={readonly}
                onToggle={(name) =>
                  setDraft((d) => ({
                    ...d,
                    guard: {
                      ...d.guard,
                      [list]: d.guard[list].includes(name)
                        ? d.guard[list].filter((x) => x !== name)
                        : [...d.guard[list], name],
                    },
                  }))
                }
              />
            );
          })}
          <p className="text-xs text-muted-foreground">{t("modelSheet.guardrails.hint")}</p>
        </FormSection>

        {/* the preview is the draft as it would be saved, which a read-only
            route never is (#1982) */}
        {!readonly && (
          <>
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
                value={buildPreview(
                  draft,
                  providerName,
                  advancedPayload,
                  seededRatesRef.current,
                  mode === "edit",
                )}
                language="json"
                label={t("modelSheet.configPreview")}
                maxHeight={280}
                density="compact"
              />
            </FormSection>
          </>
        )}
      </SheetBody>

      <SheetFooter>
        {summary && (
          // one line, and the only place the count lives: each problem is
          // already said at its own field, so this does not say it again. the
          // primary action stays enabled — it is the press that brings the
          // errors up, and focus has gone to the first of them (#2810). a
          // status, not an alert: the fields announce themselves (#2734)
          <p
            role="status"
            className="px-[22px] pt-2.5 text-xs leading-snug text-[color:var(--status-danger-text)]"
          >
            {t("modelSheet.errors.summary", { count: problems.length })}
          </p>
        )}
        <SheetError
          message={
            save.isError && !refusal
              ? advancedRejected
                ? t("modelSheet.advancedRejected", { message: errorText(save.error, t) })
                : errorText(save.error, t)
              : undefined
          }
        />
        {/* no connection check here until the provider probe can say whether
            it serves this upstream model (#2008, #2009). the provider's own
            test answers "does the provider answer", which beside a model name
            reads as a claim about the model */}
        <SheetActions>
          {/* a read-only view has nothing to discard, so it offers one way out
              rather than a Cancel beside a Close that do the same (#2046) */}
          {readonly ? (
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.close")}
            </Button>
          ) : (
            <>
              <Button variant="ghost" disabled={locked} onClick={close}>
                {t("common.cancel")}
              </Button>
              <Button disabled={editLoading || save.isPending} onClick={requestSave}>
                {cta}
              </Button>
            </>
          )}
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
