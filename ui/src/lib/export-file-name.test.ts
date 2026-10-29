import { describe, expect, it } from "bun:test";

import { exportFileName } from "@/lib/export-file-name";

// the same cases as `a_file_names_its_window_and_currency` in
// crates/rolter-control/src/analytics_export.rs, so the two names agree
describe("exportFileName (#3498)", () => {
  it("names the window and the currency the way the server does", () => {
    expect(
      exportFileName(
        "cost-attribution-business_unit",
        { since: "2026-10-01T00:00:00Z", until: "2026-11-01" },
        "eur",
      ),
    ).toBe("cost-attribution-business_unit_2026-10-01_to_2026-11-01_EUR.csv");
    expect(exportFileName("request-log", { until: "" }, "USD")).toBe(
      "request-log_7-days-ago_to_now_USD.csv",
    );
  });

  it("names a dashboard window by the dates of the instants it sends", () => {
    // what `windowBounds` sends for a range in a zone behind UTC
    expect(
      exportFileName(
        "request-log",
        { since: "2026-10-01T04:00:00.000Z", until: "2026-10-08T04:00:00.000Z" },
        "USD",
      ),
    ).toBe("request-log_2026-10-01_to_2026-10-08_USD.csv");
    // and a named window, which runs up to now
    expect(exportFileName("request-log", { since: "2026-10-28T09:15:00.000Z" }, "USD")).toBe(
      "request-log_2026-10-28_to_now_USD.csv",
    );
  });

  it("lets nothing a bound or a code holds into the name", () => {
    const name = exportFileName("x", { since: '"; rm -rf /' }, 'U"S\r\nD');
    expect(name).not.toMatch(/["; /\r\n]/);
  });
});
