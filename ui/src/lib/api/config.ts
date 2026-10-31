import { getJson, getText } from "./request";

export interface TargetDto {
  provider: string;
  model?: string | null;
  weight: number;
  /** the failure classes this target is a fallback for (#1584); absent for an ordinary target */
  takes_over?: string[];
}

/** who owns a row of the effective config; absent on a config-file row */
export interface TenancyDto {
  org_id: string;
  project_id?: string | null;
}

export interface RouteDto {
  model: string;
  /** the organization (and project) the route belongs to; absent for a config-file route */
  tenancy?: TenancyDto | null;
  strategy: string;
  targets: TargetDto[];
  /** A/B variants; a route with none serves from `targets` alone */
  variants?: { name: string; targets: TargetDto[] }[];
}

export interface VirtualKeyDto {
  key: string;
  name?: string | null;
  models: string[];
}

export interface ProviderDto {
  name: string;
  /** the organization the provider belongs to; absent for a config-file provider */
  tenancy?: TenancyDto | null;
  kind: string;
  api_base: string;
}

export interface GatewayConfigDto {
  providers: ProviderDto[];
  routes: RouteDto[];
  virtual_keys: VirtualKeyDto[];
  /**
   * every other section of `GatewayConfig` (`crates/rolter-core/src/config.rs`)
   * — server, tls, cache, guardrails, feature_flags, … — which the Effective
   * config screen renders generically rather than typing one by one (#1204)
   */
  [section: string]: unknown;
}

export function fetchConfig(): Promise<GatewayConfigDto> {
  return getJson<GatewayConfigDto>("/api/v1/config");
}

/** What kind of entry `/api/v1/config/problems` reports as left out of the snapshot whole. */
export type OmittedKind = "provider" | "route" | "virtual_key" | "public_example_key";

/**
 * One provider, route or config-file key the gateway's snapshot leaves out, so
 * it is not served (#1876).
 *
 * `index` is the row's position in the matching list of `GET /api/v1/config`
 * (`providers`, `routes` or `virtual_keys`). That response carries no tenancy,
 * so two scopes can own a route for one model and two file keys can share a
 * name; a name alone would mark both.
 */
export interface OmittedEntry {
  kind: OmittedKind;
  /** a provider name, a route model or a key's `name` (UNNAMED_KEY without one) */
  name: string;
  /** why, in English whatever the dashboard's language */
  reason: string;
  index: number;
}

/** What a file key without a `name` is called in an omitted entry. */
// escaped so check:literals does not read the angle brackets as markup text
export const UNNAMED_KEY = "\u003cunnamed>";

export interface ConfigProblems {
  /** one English sentence per finding, for a person to read and never to parse */
  problems: string[];
  omitted: OmittedEntry[];
}

/**
 * Config entries the control plane is not serving to gateways, and why (#926).
 *
 * A malformed provider is dropped from the snapshot rather than withholding the
 * whole fleet's config, so without this nothing would say it stopped being
 * served — every gateway keeps running its last good config and the change
 * simply never takes effect.
 */
export function fetchConfigProblems(): Promise<ConfigProblems> {
  return getJson<Partial<ConfigProblems>>("/api/v1/config/problems").then((r) => ({
    problems: r.problems ?? [],
    // an older control plane answers sentences only
    omitted: r.omitted ?? [],
  }));
}

/**
 * The omitted entries of `kinds`, keyed by the row they point at, for a table
 * that lists `GET /api/v1/config`'s rows in order.
 *
 * `rows` are those names in list order: an entry marks the row at its `index`
 * only while that row still carries its `name`, so a write between the two
 * reads marks nothing rather than a neighbour.
 */
export function omittedRows(
  omitted: readonly OmittedEntry[] | undefined,
  kinds: readonly OmittedKind[],
  rows: readonly string[],
): Map<number, OmittedEntry> {
  const out = new Map<number, OmittedEntry>();
  for (const entry of omitted ?? []) {
    if (kinds.includes(entry.kind) && rows[entry.index] === entry.name) {
      out.set(entry.index, entry);
    }
  }
  return out;
}

/**
 * The route models the gateway is not given at all (#1853).
 *
 * `models` is one name per route row, from `GET /api/v1/models`, which neither
 * carries tenancy nor lists every row. A name is unserved only while the
 * omitted routes bearing it cover every row listed, so a twin another scope
 * still serves keeps the model selectable. A provider dropped for its own
 * defect needs no entry of its own: a route left with no target on a known
 * provider is omitted after it, and reported as such.
 */
export function unservedModels(
  omitted: readonly OmittedEntry[] | undefined,
  models: readonly string[],
): Set<string> {
  const dropped = new Map<string, number>();
  for (const entry of omitted ?? []) {
    if (entry.kind === "route") dropped.set(entry.name, (dropped.get(entry.name) ?? 0) + 1);
  }
  const listed = new Map<string, number>();
  for (const model of models) listed.set(model, (listed.get(model) ?? 0) + 1);
  const out = new Set<string>();
  for (const [name, count] of dropped) {
    if (count >= (listed.get(name) ?? 0)) out.add(name);
  }
  return out;
}

/** What `GET /api/v1/config/export` can be narrowed by (#3440, #3396). */
export interface ConfigExportOptions {
  /** one organization's slug; absent exports the whole deployment */
  org?: string;
  /** leave caps on a person, which name an account by email, out of the file */
  withoutPersonCaps?: boolean;
}

/**
 * The stable `error.code` of the refusal to export the whole deployment while
 * two organizations share a name: the caller exports one organization instead
 * (`crates/rolter-control/src/config_export.rs`, #3480).
 */
export const CONFIG_EXPORT_NEEDS_ORG = "config_export_needs_org";

/**
 * The deployment's configuration as an importable `rolter.toml` (#1082), or
 * one organization's with `org` (#3440).
 *
 * Text rather than a record: the endpoint renders a document
 * (`crates/rolter-control/src/config_export.rs`), and the dashboard hands it
 * to the browser unchanged rather than parsing a file it only means to save.
 * Superadmin-only — one document spans every org — so the caller gates on
 * `config_export:read` before it ever asks.
 */
export function exportConfigToml(options: ConfigExportOptions = {}): Promise<string> {
  const query = new URLSearchParams();
  if (options.org) query.set("org", options.org);
  if (options.withoutPersonCaps) query.set("without_person_caps", "true");
  const suffix = query.size > 0 ? `?${query}` : "";
  return getText(`/api/v1/config/export${suffix}`);
}
