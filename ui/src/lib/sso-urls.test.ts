import { describe, expect, it } from "bun:test";

import { ssoRedirectUri } from "./api";

// the add sheet previews a provider's redirect uri before the provider exists,
// so this has to build exactly what `redirect_uri` in
// crates/rolter-control/src/sso.rs will build once it does (#2083)
describe("ssoRedirectUri", () => {
  it("appends the callback path to the deployment's public base", () => {
    // the same pair `redirect_uri_comes_from_configuration_not_the_request`
    // pins on the server
    expect(ssoRedirectUri("https://rolter.example.com", "keycloak")).toBe(
      "https://rolter.example.com/auth/sso/keycloak/callback",
    );
  });

  it("keeps a base that carries a port or a path prefix", () => {
    expect(ssoRedirectUri("http://localhost:4001", "okta")).toBe(
      "http://localhost:4001/auth/sso/okta/callback",
    );
    expect(ssoRedirectUri("https://gw.example.com/rolter", "entra")).toBe(
      "https://gw.example.com/rolter/auth/sso/entra/callback",
    );
  });
});
