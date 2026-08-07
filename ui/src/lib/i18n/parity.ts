// catalog parity rules, shared by the unit test and the CI gate
// (`scripts/check-i18n.ts`) so both hold translations to exactly one standard.

export type Catalog = { [key: string]: string | Catalog };

/** nested catalog -> flat `a.b.c` -> value */
export function flatten(node: Catalog, prefix = ""): Map<string, string> {
  const out = new Map<string, string>();
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.set(path, value);
    else for (const [k, v] of flatten(value, path)) out.set(k, v);
  }
  return out;
}

// i18next plural keys are `base_<category>`. the categories a locale needs are
// fixed by CLDR, not by the base catalog: english has one/other, russian needs
// one/few/many/other. parity is therefore compared on the *base* key, and each
// locale is then held to exactly the categories Intl says it must provide.
const CATEGORIES = ["zero", "one", "two", "few", "many", "other"];

export function splitPlural(key: string): { base: string; category: string | null } {
  const at = key.lastIndexOf("_");
  if (at === -1) return { base: key, category: null };
  const suffix = key.slice(at + 1);
  return CATEGORIES.includes(suffix)
    ? { base: key.slice(0, at), category: suffix }
    : { base: key, category: null };
}

/** base key -> plural categories present for it (empty set = a plain key) */
export function byBase(catalog: Map<string, string>): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const key of catalog.keys()) {
    const { base, category } = splitPlural(key);
    if (!out.has(base)) out.set(base, new Set());
    if (category) out.get(base)!.add(category);
  }
  return out;
}

// {{name}} interpolations and <0>…</0> Trans slots both have to survive a
// translation for the rendered string to come out intact
export function placeholders(value: string): Set<string> {
  return new Set([...(value.match(/\{\{[^}]+\}\}/g) ?? []), ...(value.match(/<\d+>/g) ?? [])]);
}

export interface Divergence {
  missing: string[];
  orphaned: string[];
  plural: string[];
  placeholder: string[];
  empty: string[];
}

export function isClean(d: Divergence): boolean {
  return (
    d.missing.length + d.orphaned.length + d.plural.length + d.placeholder.length + d.empty.length ===
    0
  );
}

/** every way `catalog` can diverge from the base catalog for `locale` */
export function compare(
  base: Map<string, string>,
  catalog: Map<string, string>,
  locale: string,
): Divergence {
  const baseBases = byBase(base);
  const localeBases = byBase(catalog);

  const missing = [...baseBases.keys()].filter((k) => !localeBases.has(k));
  const orphaned = [...localeBases.keys()].filter((k) => !baseBases.has(k));

  // a plural key in the base must be plural here too, with exactly the
  // categories this locale's rules call for — no more, no fewer
  const wanted = [...new Intl.PluralRules(locale).resolvedOptions().pluralCategories].sort();
  const plural: string[] = [];
  for (const [key, categories] of baseBases) {
    const mine = localeBases.get(key);
    if (!mine || categories.size === 0) continue;
    const have = [...mine].sort();
    if (wanted.join(",") !== have.join(",")) {
      plural.push(`${key} (needs ${wanted.join("/")}; has ${have.join("/") || "none"})`);
    }
  }

  const placeholder: string[] = [];
  const empty: string[] = [];
  for (const [key, value] of catalog) {
    if (value.trim() === "") empty.push(key);
    // a plural variant is checked against whichever base variant exists
    const { base: stem } = splitPlural(key);
    const expected = base.get(key) ?? base.get(`${stem}_other`) ?? base.get(stem);
    if (expected === undefined) continue;
    const want = placeholders(expected);
    const got = placeholders(value);
    const lost = [...want].filter((p) => !got.has(p));
    const extra = [...got].filter((p) => !want.has(p));
    if (lost.length || extra.length) {
      placeholder.push(
        `${key} (missing ${lost.join(", ") || "—"}; unexpected ${extra.join(", ") || "—"})`,
      );
    }
  }

  return { missing, orphaned, plural, placeholder, empty };
}
