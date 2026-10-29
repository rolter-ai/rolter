import { describe, expect, it } from "bun:test";

import { ApiError } from "@/lib/api";
import { describeCodedError, describeError, errorText, KNOWN_ERROR_CODES } from "@/lib/error-copy";
import { GatewayError } from "@/lib/gateway";
import { flatten } from "@/lib/i18n/parity";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

// echoes the key, so the assertion names the line chosen rather than its prose
const t = ((key: string) => key) as never;

describe("describeError", () => {
  it("translates a missing deployment KEK independently of server wording", () => {
    expect(describeError(new ApiError("sealing unavailable", 503, "kek_unavailable"), t)).toEqual({
      message: "errors.api.codes.kek_unavailable",
    });
  });
  it("translates a known code and never shows the server's message", () => {
    const copy = describeError(
      new ApiError("last superadmin (internal)", 409, "last_superadmin"),
      t,
    );
    expect(copy).toEqual({ message: "errors.api.codes.last_superadmin" });
  });

  it("translates both timeout codes rather than the server's words (#3147)", () => {
    for (const code of ["request_timeout", "store_timeout"]) {
      expect(describeError(new ApiError("timed out (internal)", 504, code), t)).toEqual({
        message: `errors.api.codes.${code}`,
      });
    }
  });

  it("keeps an invalid_field message as detail, since only it names the field", () => {
    const copy = describeError(new ApiError("slug must match ^[a-z0-9]", 400, "invalid_field"), t);
    expect(copy).toEqual({
      message: "errors.api.codes.invalid_field",
      detail: "slug must match ^[a-z0-9]",
    });
  });

  it("translates invalid_body and keeps the parser's words as detail", () => {
    const copy = describeError(
      new ApiError("invalid request body: invalid type: integer `7`", 400, "invalid_body"),
      t,
    );
    expect(copy).toEqual({
      message: "errors.api.codes.invalid_body",
      detail: "invalid request body: invalid type: integer `7`",
    });
    expect(KNOWN_ERROR_CODES).toContain("invalid_body");
  });

  it("translates a taken name without the server's words", () => {
    const copy = describeError(new ApiError("provider name 'x' is in use", 409, "name_taken"), t);
    expect(copy).toEqual({ message: "errors.api.codes.name_taken" });
  });

  it("translates no_local_password, the 409 an SSO-only account gets on a password change", () => {
    const copy = describeError(
      new ApiError("this account signs in through single sign-on", 409, "no_local_password"),
      t,
    );
    expect(copy).toEqual({ message: "errors.api.codes.no_local_password" });
    expect(KNOWN_ERROR_CODES).toContain("no_local_password");
  });

  it("falls back to the status for an uncoded 401, 403, 429 and 5xx", () => {
    expect(describeError(new ApiError("x", 401), t).message).toBe("errors.api.unauthorized");
    expect(describeError(new ApiError("x", 403), t).message).toBe("errors.api.forbidden");
    expect(describeError(new ApiError("x", 429), t).message).toBe("errors.api.rateLimited");
    expect(describeError(new ApiError("pg: boom", 500), t)).toEqual({
      message: "errors.api.server",
    });
  });

  it("keeps an unknown message as detail under a generic line", () => {
    const copy = describeError(new ApiError("name must not be empty", 400, "brand_new"), t);
    expect(copy).toEqual({ message: "errors.api.generic", detail: "name must not be empty" });
  });

  it("reads a thrown non-ApiError as unreachable", () => {
    expect(describeError(new TypeError("Failed to fetch"), t).message).toBe(
      "errors.api.unreachable",
    );
  });

  it("keeps a gateway message as detail", () => {
    expect(describeError(new GatewayError("model overloaded", 503), t).detail).toBe(
      "model overloaded",
    );
  });
});

describe("error catalog", () => {
  it("has a line for every known code in every locale", () => {
    for (const catalog of [en, ru]) {
      const keys = flatten(catalog as never);
      for (const code of KNOWN_ERROR_CODES) {
        expect([...keys.keys()]).toContain(`errors.api.codes.${code}`);
      }
    }
  });
});

describe("invalid_field naming", () => {
  it("names the field in the translated line when the control plane sent one", () => {
    const copy = describeError(
      new ApiError(
        "'x y' is not a valid header name",
        400,
        "invalid_field",
        undefined,
        "forwarded_headers[2]",
      ),
      t,
    );
    expect(copy).toEqual({
      message: "errors.api.invalidFieldNamed",
      detail: "'x y' is not a valid header name",
    });
  });
});

describe("errorText", () => {
  it("is the translated line alone for a coded refusal", () => {
    expect(errorText(new ApiError("taken", 409, "name_taken"), t)).toBe(
      "errors.api.codes.name_taken",
    );
  });

  it("appends the server's words where they are kept as detail", () => {
    expect(errorText(new ApiError("slug is bad", 400), t)).toBe("errors.api.generic slug is bad");
  });
});

describe("describeCodedError", () => {
  it("is null for anything but a coded ApiError the catalog knows", () => {
    expect(describeCodedError(new Error("plain"), t)).toBeNull();
    expect(describeCodedError("a string", t)).toBeNull();
    expect(describeCodedError(new ApiError("no code", 409), t)).toBeNull();
    expect(describeCodedError(new ApiError("new code", 409, "not_published"), t)).toBeNull();
  });

  it("translates a known code and leaves the server's words out", () => {
    expect(describeCodedError(new ApiError("english", 409, "api_token_inactive"), t)).toEqual({
      message: "errors.api.codes.api_token_inactive",
    });
  });

  it("keeps the server's words for the codes whose sentence cannot name the thing", () => {
    for (const code of ["invalid_field", "invalid_body", "referenced", "scope_mismatch"]) {
      expect(describeCodedError(new ApiError("names the dependents", 409, code), t)).toMatchObject({
        detail: "names the dependents",
      });
    }
  });
});
