import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FilterCheckList } from "@/components/ui/filter-panel";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Tag } from "@/components/ui/tag";
import { fetchRoutes } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";

// the name + expiry + reach block shared by the two screens that mint a virtual
// key: the admin one (Keys) and the self-service one (Account). #945 made both
// of those choices required, and a rule enforced on one screen only is a rule
// with a way around it.

/** presets the expiry picker offers, in days; `null` is the deliberate "never" */
export const KEY_TTL_CHOICES: (number | null)[] = [7, 30, 60, 90, null];
/** what an operator gets if they change nothing — finite, by design */
export const DEFAULT_KEY_TTL_DAYS = 30;
/** mirrors `MAX_KEY_NAME_LEN` in `crates/rolter-control/src/me.rs` */
export const MAX_KEY_NAME_LEN = 64;
/** the select's "never" token: its own value, so an unset control can never be
 *  mistaken for a chosen "never" */
export const NEVER = "never";

/** `undefined` means "never expires", which is what the API wants omitted */
export function ttlToDays(ttl: string): number | undefined {
  return ttl === NEVER ? undefined : Number(ttl);
}

/** the same rule the control plane applies, checked here so the operator learns
 *  it before spending a round trip */
export function keyNameProblem(name: string): "blank" | "long" | null {
  const trimmed = name.trim();
  if (trimmed.length === 0) return "blank";
  if (trimmed.length > MAX_KEY_NAME_LEN) return "long";
  return null;
}

export function KeyNameField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  const tooLong = keyNameProblem(value) === "long";
  return (
    <Field
      label={t("keyMint.name")}
      error={tooLong ? t("keyMint.nameTooLong", { max: MAX_KEY_NAME_LEN }) : undefined}
      hint={tooLong ? undefined : t("keyMint.nameHint")}
    >
      <Input
        required
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={t("keyMint.namePlaceholder")}
      />
    </Field>
  );
}

export function KeyExpiryField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <Field
      label={t("keyMint.expiry")}
      hint={value === NEVER ? t("keyMint.expiryNeverWarning") : t("keyMint.expiryHint")}
    >
      <Select value={value} onChange={(e) => onChange(e.target.value)}>
        {KEY_TTL_CHOICES.map((days) => (
          <option key={days ?? NEVER} value={days === null ? NEVER : String(days)}>
            {days === null
              ? t("keyMint.expiryNever")
              : t("keyMint.expiryDays", { count: days })}
          </option>
        ))}
      </Select>
    </Field>
  );
}

/** what `useRouteModels` hands the allow-list field */
export interface RouteModels {
  /** the model addresses the project routes, deduplicated and sorted */
  models: string[];
  loading: boolean;
  error: unknown;
  retry: () => void;
}

/**
 * The models a project can route to, for the allow-list to tick off.
 *
 * `retry: false` on purpose: a member without route read access gets a 403
 * that asking again will not improve, and the field is deliberately usable
 * without the list — it falls back to typing an address by hand.
 */
export function useRouteModels(
  projectId: string | undefined,
  /** false while the sheet is closed: nothing is there to populate yet */
  enabled = true,
): RouteModels {
  const query = useQuery({
    queryKey: ["routes", projectId],
    queryFn: () => fetchRoutes(projectId as string),
    enabled: enabled && !!projectId,
    retry: false,
  });
  const models = React.useMemo(
    () => [...new Set((query.data ?? []).map((r) => r.model))].sort(),
    [query.data],
  );
  return {
    models,
    loading: query.isLoading,
    error: query.error,
    retry: () => void query.refetch(),
  };
}

/**
 * The models a key may ask for, ticked off the project's own routes.
 *
 * It used to be a comma-separated text box, which meant an operator had to
 * know and spell every model address correctly and a typo produced an
 * allow-list matching nothing — silently, since the control plane stores the
 * strings as given (#1345). The routes the project actually serves are the
 * list now, and the wire format is unchanged: the same array of addresses.
 *
 * Free-form entry stays, because an allow-list may legitimately name an
 * address this project does not route today — one a bootstrap-config route
 * serves, or one a route about to be created will. For the same reason an
 * address already on the key that no route offers is kept and shown as a
 * custom entry rather than dropped: dropping it would quietly widen the key.
 */
export function KeyModelsField({
  value,
  onChange,
  options = [],
  loading = false,
  error,
  onRetry,
}: {
  /** the allow-list exactly as it goes on the wire */
  value: string[];
  onChange: (value: string[]) => void;
  /** model addresses the project routes, offered as ticks */
  options?: string[];
  /** the route lookup is still in flight */
  loading?: boolean;
  /** the route lookup failed; free-form entry carries on regardless */
  error?: unknown;
  onRetry?: () => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = React.useState("");
  // the label names the free-form input rather than whichever tick or chip
  // happens to come first in the DOM
  const inputId = React.useId();
  const custom = value.filter((m) => !options.includes(m));
  const offered = [...options, ...custom];

  const toggle = (model: string) =>
    onChange(
      value.includes(model) ? value.filter((m) => m !== model) : [...value, model],
    );

  // a paste of the old comma-separated form still lands as several entries,
  // so nothing an operator already has written down stops working
  const addDraft = () => {
    const added = parseModels(draft).filter((m) => !value.includes(m));
    if (added.length > 0) onChange([...value, ...added]);
    setDraft("");
  };

  return (
    <Field
      label={t("keyMint.models")}
      htmlFor={inputId}
      hint={
        value.length === 0
          ? t("keyMint.modelsAll")
          : t("keyMint.modelsSome", { count: value.length })
      }
    >
      <div className="space-y-2">
        {value.length > 0 && (
          <ul aria-label={t("keyMint.modelsSelected")} className="flex flex-wrap gap-1.5">
            {value.map((model) => (
              <li key={model}>
                <Tag
                  removeLabel={t("keyMint.modelsRemove", { model })}
                  onRemove={() => toggle(model)}
                >
                  {model}
                </Tag>
              </li>
            ))}
          </ul>
        )}
        {loading ? (
          <ListSkeleton rows={3} />
        ) : error ? (
          <LoadError error={error} resource={t("errors.resources.routes")} onRetry={onRetry} />
        ) : offered.length === 0 ? (
          <EmptyState
            className="rounded-md border border-dashed border-border py-6"
            uxTarget="key-model-allowlist"
            title={t("keyMint.modelsEmpty")}
            description={t("keyMint.modelsEmptyBody")}
          />
        ) : (
          <FilterCheckList
            options={offered.map((model) => ({
              value: model,
              label: options.includes(model) ? (
                model
              ) : (
                <span className="flex min-w-0 items-center gap-1.5">
                  <span className="min-w-0 truncate">{model}</span>
                  <Badge tone="outline">{t("keyMint.modelsCustom")}</Badge>
                </span>
              ),
            }))}
            selected={value}
            onChange={onChange}
          />
        )}
        <div className="flex gap-2">
          <Input
            id={inputId}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              // enter inside a sheet would otherwise submit the form with a
              // half-typed address still in the box
              if (e.key !== "Enter") return;
              e.preventDefault();
              addDraft();
            }}
            placeholder={t("keyMint.modelsPlaceholder")}
          />
          <Button
            type="button"
            variant="outline"
            disabled={parseModels(draft).length === 0}
            onClick={addDraft}
          >
            {t("keyMint.modelsAdd")}
          </Button>
        </div>
      </div>
    </Field>
  );
}

/**
 * The per-key response-cache override.
 *
 * Three states, not a switch: "inherit" is the absence of a decision and is
 * what a key gets when nobody made one, while "off" and "on" both override the
 * route. Collapsing that to a boolean would turn "I did not choose" into "I
 * chose no".
 */
export function KeyCacheField({
  value,
  onChange,
}: {
  value: CacheMode;
  onChange: (value: CacheMode) => void;
}) {
  const { t } = useTranslation();
  return (
    <Field label={t("keyMint.cache")} hint={t("keyMint.cacheHint")}>
      <Select
        aria-label={t("keyMint.cache")}
        value={value}
        onChange={(e) => onChange(e.target.value as CacheMode)}
      >
        <option value="inherit">{t("keyMint.cacheInherit")}</option>
        <option value="off">{t("keyMint.cacheOff")}</option>
        <option value="on">{t("keyMint.cacheOn")}</option>
      </Select>
    </Field>
  );
}

/** the three states `cache` can be in on the wire: null, false, true */
export type CacheMode = "inherit" | "off" | "on";

export function cacheMode(cache: boolean | null | undefined): CacheMode {
  if (cache === true) return "on";
  if (cache === false) return "off";
  return "inherit";
}

/** `null` is the wire value for "inherit", which is not the same as `false` */
export function parseCacheMode(value: string): boolean | null {
  if (value === "on") return true;
  if (value === "off") return false;
  return null;
}

/** split a comma-separated allow-list into the array the API takes */
export function parseModels(text: string): string[] {
  return text
    .split(",")
    .map((m) => m.trim())
    .filter(Boolean);
}

/**
 * What the key will be able to reach, stated before it exists. The secret is
 * shown exactly once, so "did I just mint something narrow, or something that
 * can spend money against every provider?" has to be answerable now.
 */
export function KeyReachSummary({
  project,
  models,
  providers = [],
  ttl,
}: {
  project: string;
  models: string[];
  /** provider slugs the key is narrowed to; empty is every provider */
  providers?: string[];
  ttl: string;
}) {
  const { t } = useTranslation();
  const format = useFormat();
  const days = ttlToDays(ttl);
  const expiryDate = days === undefined ? null : new Date(Date.now() + days * 86_400_000);

  return (
    <div className="space-y-1 rounded-md border border-dashed border-border bg-muted/40 p-3">
      <p className="text-xs font-medium text-foreground">{t("keyMint.reach.title")}</p>
      <ul className="space-y-0.5 text-xs text-muted-foreground">
        <li>{t("keyMint.reach.project", { project })}</li>
        <li>
          {models.length === 0
            ? t("keyMint.reach.allModels")
            : t("keyMint.reach.someModels", {
                count: models.length,
                models: models.join(", "),
              })}
        </li>
        <li>
          {providers.length === 0
            ? t("keyMint.reach.allProviders")
            : t("keyMint.reach.someProviders", {
                count: providers.length,
                providers: providers.join(", "),
              })}
        </li>
        <li>
          {expiryDate === null
            ? t("keyMint.reach.never")
            : t("keyMint.reach.until", { date: format.date(expiryDate) })}
        </li>
      </ul>
    </div>
  );
}
