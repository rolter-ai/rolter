import { describe, expect, it } from "bun:test";

import { scimBaseUrl } from "./api";

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
