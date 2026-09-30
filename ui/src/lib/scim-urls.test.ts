import { describe, expect, it } from "bun:test";

import { scimBaseUrl } from "./api";
import { PUBLIC_URL_QUERY_KEY } from "./use-public-url";

// the provisioning screen shows the URL an IdP's SCIM connector is pointed at.
// scim.rs mounts its resource endpoints on /scim/v2, so this has to land on
// the same path the control plane serves (#2079)
describe("scimBaseUrl", () => {
  it("appends the SCIM mount to the deployment's public base", () => {
    expect(scimBaseUrl("https://rolter.example.com")).toBe("https://rolter.example.com/scim/v2");
  });

  it("keeps a base that carries a port or a path prefix", () => {
    expect(scimBaseUrl("http://localhost:4001")).toBe("http://localhost:4001/scim/v2");
    expect(scimBaseUrl("https://gw.example.com/rolter")).toBe(
      "https://gw.example.com/rolter/scim/v2",
    );
  });
});

// the Single Sign-On screen reads the same endpoint, and the two screens only
// share one request, and agree on the unset warning, while they share one key
describe("PUBLIC_URL_QUERY_KEY", () => {
  it("is the key the Single Sign-On screen reads the public url under", async () => {
    const sso = await Bun.file(new URL("../pages/SingleSignOn.tsx", import.meta.url)).text();
    const key = JSON.stringify(PUBLIC_URL_QUERY_KEY[0]);
    expect(sso.includes(key) || sso.includes("PUBLIC_URL_QUERY_KEY")).toBe(true);
  });
});
