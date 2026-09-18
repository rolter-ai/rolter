import { describe, it, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import { parseWorkspaceVersion, readAppVersion } from "./app-version";

const WORKSPACE_MANIFEST = fileURLToPath(new URL("../../Cargo.toml", import.meta.url));

describe("app version", () => {
  it("reads the version from [workspace.package]", () => {
    expect(
      parseWorkspaceVersion(`
[workspace]
members = ["crates/*"]

[workspace.package]
version = "1.2.3"
edition = "2021"
`),
    ).toBe("1.2.3");
  });

  it("is not fooled by a dependency's version", () => {
    // [workspace.dependencies] is full of `version = "..."`, and it comes
    // first in the real manifest — matching the first one in the file would
    // label the dashboard with some crate's version
    expect(
      parseWorkspaceVersion(`
[workspace.dependencies]
serde = { version = "1.0.200" }
tokio = "1.40.0"

[workspace.package]
version = "0.0.10"
`),
    ).toBe("0.0.10");
  });

  it("throws rather than falling back to a wrong version", () => {
    // silently defaulting is exactly how the sidebar came to show v0.0.1 for
    // ten releases; a build that cannot find the version must fail loudly
    expect(() => parseWorkspaceVersion("[workspace]\nmembers = []\n")).toThrow(/workspace.package/);
  });

  it("resolves the real workspace manifest to the shipped version", () => {
    // the end-to-end guard: whatever the workspace is at, that is what the
    // dashboard reports — and it is never the stale 0.0.1
    const version = readAppVersion(WORKSPACE_MANIFEST);
    expect(version).toMatch(/^\d+\.\d+\.\d+/);
    expect(readFileSync(WORKSPACE_MANIFEST, "utf8")).toContain(`version = "${version}"`);
  });

  it("no longer keeps a second version in package.json", () => {
    // one source of truth: a version field here is one nothing bumps
    const pkg = JSON.parse(
      readFileSync(fileURLToPath(new URL("../package.json", import.meta.url)), "utf8"),
    ) as Record<string, unknown>;
    expect(pkg.version).toBeUndefined();
  });
});
