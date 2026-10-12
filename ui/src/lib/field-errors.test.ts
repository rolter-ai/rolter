import { describe, expect, it } from "bun:test";

import { ApiError } from "@/lib/api";
import { serverFieldError, serverFieldPath } from "@/lib/field-errors";

const FIELDS = { retention_days: "retention", forwarded_headers: "forwarded" } as const;

describe("serverFieldError", () => {
  it("prefers the structured field over the message prefix", () => {
    const error = new ApiError(
      "'x y' is not a valid header name",
      400,
      "invalid_field",
      undefined,
      "forwarded_headers[1]",
    );
    expect(serverFieldError(error, FIELDS)).toEqual({
      field: "forwarded",
      message: "'x y' is not a valid header name",
    });
  });

  it("lets the structured field win when the message opens with another field", () => {
    const error = new ApiError(
      "retention_days must be 1-30",
      400,
      "invalid_field",
      undefined,
      "forwarded_headers",
    );
    expect(serverFieldError(error, FIELDS)?.field).toBe("forwarded");
  });

  it("falls back to the message prefix when the refusal sends no field", () => {
    const error = new ApiError("retention_days must be between 1 and 3650", 400);
    expect(serverFieldError(error, FIELDS)?.field).toBe("retention");
  });

  it("falls back to the prefix when the field is not one the screen knows", () => {
    const error = new ApiError(
      "retention_days must be positive",
      400,
      "invalid_field",
      undefined,
      "other",
    );
    expect(serverFieldError(error, FIELDS)?.field).toBe("retention");
  });

  it("is null for a refusal that names no field of the screen, and for non-400s", () => {
    expect(
      serverFieldError(new ApiError("'x' is not a valid header name", 400), FIELDS),
    ).toBeNull();
    expect(serverFieldError(new ApiError("retention_days bad", 500), FIELDS)).toBeNull();
    expect(serverFieldError(new Error("retention_days bad"), FIELDS)).toBeNull();
  });
});

describe("serverFieldPath", () => {
  it("splits a row path into its root", () => {
    const error = new ApiError("m", 400, "invalid_field", undefined, "injected_headers.x-team");
    expect(serverFieldPath(error)).toEqual({
      path: "injected_headers.x-team",
      root: "injected_headers",
    });
    const listed = new ApiError("m", 400, "invalid_field", undefined, "forwarded_headers[2]");
    expect(serverFieldPath(listed)?.root).toBe("forwarded_headers");
  });

  it("is null without a field", () => {
    expect(serverFieldPath(new ApiError("m", 400))).toBeNull();
  });
});
