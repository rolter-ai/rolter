import { describe, expect, it } from "bun:test";
import { Glob } from "bun";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

// #2051: `main.tsx` imported the two fontsource packages and the Storybook
// preview did not, so every story rendered in fallback fonts while the app
// rendered in Geist, and nothing failed. Both entries now import one module,
// `lib/fonts.ts`. What this pins is the way that drifts back: an entry that
// stops importing it, a third place that imports a package on its own, and a
// package whose family is not the one the type tokens ask for first (a family
// nothing names is a download no text ever uses).

const UI = fileURLToPath(new URL("../../", import.meta.url));
const read = (path: string) => readFileSync(`${UI}${path}`, "utf8");

/** The side-effect imports of fontsource packages in one source file. */
function fontsourceImports(source: string): string[] {
  return [...source.matchAll(/^import\s+["'](@fontsource(?:-variable)?\/[^"']+)["'];?$/gm)].map(
    (match) => match[1],
  );
}

/** The first family a `--font-*` token in `index.css` names. */
function tokenFamily(css: string, token: string): string | undefined {
  const stack = css.match(new RegExp(`${token}:\\s*([^;]+);`))?.[1];
  return stack
    ?.split(",")[0]
    ?.trim()
    .replace(/^["']|["']$/g, "");
}

describe("the vendored fonts", () => {
  const packages = fontsourceImports(read("src/lib/fonts.ts"));

  it("are imported by lib/fonts.ts, so the check below cannot pass by finding nothing", () => {
    expect(packages.length).toBeGreaterThan(0);
  });

  it("reach both the app and the Storybook preview through that one module", () => {
    expect(read("src/main.tsx")).toMatch(/^import\s+["']@\/lib\/fonts["'];$/m);
    expect(read(".storybook/preview.ts")).toMatch(/^import\s+["']\.\.\/src\/lib\/fonts["'];$/m);
  });

  it("are imported nowhere else, so a second list cannot grow beside it", () => {
    const elsewhere: string[] = [];
    for (const pattern of ["src/**/*.{ts,tsx}", ".storybook/*.ts"]) {
      for (const path of new Glob(pattern).scanSync({ cwd: UI, dot: true })) {
        if (path === "src/lib/fonts.ts") continue;
        if (fontsourceImports(read(path)).length > 0) elsewhere.push(path);
      }
    }
    expect(elsewhere).toEqual([]);
  });

  it("declare exactly the families --font-sans and --font-mono ask for first", () => {
    const declared = packages.map((name) => {
      const css = read(`node_modules/${name}/index.css`);
      return css.match(/font-family:\s*["']([^"']+)["']/)?.[1];
    });
    const css = read("src/index.css");
    expect(declared.sort()).toEqual(
      [tokenFamily(css, "--font-sans"), tokenFamily(css, "--font-mono")].sort(),
    );
  });
});
