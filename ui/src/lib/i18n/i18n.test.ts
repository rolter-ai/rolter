import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";

import { leafKeys } from "@/lib/nav";

import en from "./locales/en.json";
import ru from "./locales/ru.json";
import i18n, {
  DEFAULT_LOCALE,
  LOCALES,
  LOCALE_NAMES,
  currentLocale,
  detectLocale,
  setLocale,
} from "./index";
import { formattersFor } from "./format";
import { compare, flatten, type Catalog } from "./parity";

// bun's runtime ships no localStorage (which is why the source wraps every
// access in try/catch), and the api/ux suites install stubs of their own on
// globalThis from *their* beforeEach. bun runs every test file in one process,
// so a stub installed once here gets clobbered by whichever file runs next —
// reinstall per test instead, and hand the global back when the file is done.
const originalStorage = globalThis.localStorage;

function installStorage() {
  const store = new Map<string, string>();
  globalThis.localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: () => null,
    get length() {
      return store.size;
    },
  } as unknown as Storage;
}

beforeEach(installStorage);

afterAll(() => {
  globalThis.localStorage = originalStorage;
});

const flatEn = flatten(en as Catalog);
const flatRu = flatten(ru as Catalog);

describe("catalogs", () => {
  // the same rules the CI gate runs (scripts/check-i18n.ts), so a translation
  // cannot drift past `bun test` and get caught only in CI
  const divergence = compare(flatEn, flatRu, "ru");

  test("ru is missing no key from the base catalog", () => {
    expect(divergence.missing).toEqual([]);
  });

  test("ru carries no key the base catalog dropped", () => {
    expect(divergence.orphaned).toEqual([]);
  });

  test("ru supplies every plural form russian requires", () => {
    // russian needs one/few/many/other where english needs only one/other
    expect(divergence.plural).toEqual([]);
  });

  test("interpolation placeholders survive translation", () => {
    expect(divergence.placeholder).toEqual([]);
  });

  test("no catalog leaves an empty string", () => {
    expect(divergence.empty).toEqual([]);
    for (const [key, value] of flatEn) {
      expect(value.trim(), `empty value for ${key}`).not.toBe("");
    }
  });

  test("every routable screen has a label and header copy", () => {
    // leaves are the keys that resolve to a route; parents are pure toggles.
    // a leaf missing either entry renders its raw key in the ui
    const nav = (en as Catalog).nav as Catalog;
    const screens = (en as Catalog).screens as Catalog;
    for (const key of leafKeys()) {
      expect(nav[key], `no nav.${key} entry`).toBeDefined();
      expect(screens[key], `no screens.${key} entry`).toBeDefined();
    }
  });

  test("no orphaned nav label", () => {
    // a nav entry with no matching NavDef is copy nothing renders
    const known = new Set<string>(leafKeys());
    const parents = new Set(["observability", "models", "mcp", "alerting", "governance",
      "guardrails", "adaptive-routing", "settings"]);
    for (const key of Object.keys((en as Catalog).nav as Catalog)) {
      expect(known.has(key) || parents.has(key), `nav.${key} matches no nav entry`).toBe(true);
    }
  });

  test("language names are identical across catalogs", () => {
    // a language is always listed in its own language, so it is findable even
    // when the ui is showing one you cannot read
    for (const locale of LOCALES) {
      expect(flatRu.get(`locale.${locale}`)).toBe(flatEn.get(`locale.${locale}`));
      expect(flatEn.get(`locale.${locale}`)).toBe(LOCALE_NAMES[locale]);
    }
  });
});

describe("detectLocale", () => {
  const originalNavigator = globalThis.navigator;

  afterEach(() => {
    Object.defineProperty(globalThis, "navigator", {
      value: originalNavigator,
      configurable: true,
    });
  });

  function stubNavigator(languages: string[]) {
    Object.defineProperty(globalThis, "navigator", {
      value: { languages, language: languages[0] },
      configurable: true,
    });
  }

  test("a stored choice beats the browser", () => {
    stubNavigator(["en-US"]);
    localStorage.setItem("rolter.locale", "ru");
    expect(detectLocale()).toBe("ru");
  });

  test("an unsupported stored value is ignored", () => {
    stubNavigator(["ru-RU"]);
    localStorage.setItem("rolter.locale", "kl");
    expect(detectLocale()).toBe("ru");
  });

  test("regional browser tags match on the primary subtag", () => {
    stubNavigator(["ru-RU"]);
    expect(detectLocale()).toBe("ru");
  });

  test("the first supported browser preference wins", () => {
    stubNavigator(["kl-GL", "ru", "en"]);
    expect(detectLocale()).toBe("ru");
  });

  test("english is the floor when nothing matches", () => {
    stubNavigator(["kl-GL"]);
    expect(detectLocale()).toBe(DEFAULT_LOCALE);
  });
});

describe("setLocale", () => {
  afterEach(async () => {
    await setLocale(DEFAULT_LOCALE);
  });

  test("loads the lazy catalog and translates without a reload", async () => {
    expect(i18n.t("shell.signOut")).toBe("Sign out");
    await setLocale("ru");
    expect(currentLocale()).toBe("ru");
    expect(i18n.t("shell.signOut")).toBe("Выйти");
    // the screen header reads through the same catalog
    expect(i18n.t("screens.dashboard.title")).toBe("Обзор");
  });

  test("interpolation is applied in the active locale", async () => {
    await setLocale("ru");
    expect(i18n.t("shell.roleWithOrg", { org: "Acme" })).toBe("Администратор · Acme");
  });

  test("the choice is persisted for the next visit", async () => {
    await setLocale("ru");
    expect(localStorage.getItem("rolter.locale")).toBe("ru");
  });
});

describe("formatters", () => {
  test("numbers follow the active locale, not the browser", () => {
    // ru groups with a narrow no-break space and uses a decimal comma
    expect(formattersFor("en").number(1234567.5)).toBe("1,234,567.5");
    const russian = formattersFor("ru").number(1234567.5);
    expect(russian).toContain(",");
    expect(russian).not.toContain("1,234,567.5");
  });

  test("sub-cent costs keep enough precision to be non-zero", () => {
    expect(formattersFor("en").currency(0.0004)).toBe("$0.0004");
    expect(formattersFor("en").currency(12.5)).toBe("$12.50");
  });

  test("percent formatting is stable", () => {
    expect(formattersFor("en").percent(0.734)).toBe("73.4%");
  });
});
