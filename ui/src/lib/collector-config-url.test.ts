import { describe, expect, it } from "bun:test";

import { collectorConfigUrl } from "./api";

// the collector-config dialog shows the address the document is served from, so
// this has to land on the path collector_config.rs mounts, under the public
// base the control plane reports (#2106)
describe("collectorConfigUrl", () => {
  it("appends the endpoint to the deployment's public base", () => {
    expect(collectorConfigUrl("https://rolter.example.com")).toBe(
      "https://rolter.example.com/api/v1/connectors/collector-config",
    );
  });

  it("keeps a base that carries a port or a path prefix", () => {
    expect(collectorConfigUrl("http://localhost:4001")).toBe(
      "http://localhost:4001/api/v1/connectors/collector-config",
    );
    expect(collectorConfigUrl("https://gw.example.com/rolter")).toBe(
      "https://gw.example.com/rolter/api/v1/connectors/collector-config",
    );
  });
});
