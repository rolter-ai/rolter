import {
  MCP_TOOLS_DISCOVERY_ERRORS,
  type McpServerRow,
  type McpToolsDiscoveryError,
} from "@/lib/api";

/** How often the server list is re-read while a discovery is still outstanding. */
export const DISCOVERY_POLL_MS = 2000;

/** The card's discovery state in words the screen can branch on. */
export type DiscoveryView = "pending" | "declaresNone" | "discovered" | "failed";

/**
 * `declaresNone` is its own view so an answered-but-empty manifest never reads
 * as "not discovered yet": only a `discovered` row with no tools is one.
 */
export function discoveryView(
  server: Pick<McpServerRow, "tools_discovery_status" | "tools">,
): DiscoveryView {
  if (server.tools_discovery_status === "failed") return "failed";
  if (server.tools_discovery_status === "pending") return "pending";
  return server.tools.length ? "discovered" : "declaresNone";
}

/**
 * The i18n key (under `pages.mcpCatalog.discovery.reasons`) for an error code.
 * A code this build does not know falls back to `unknown`, so a newer control
 * plane never prints a raw code or an empty reason.
 */
export function discoveryReasonKey(code: McpToolsDiscoveryError | string | null): string | null {
  if (code === null) return null;
  const known = (MCP_TOOLS_DISCOVERY_ERRORS as readonly string[]).includes(code);
  return `pages.mcpCatalog.discovery.reasons.${known ? code : "unknown"}`;
}

/**
 * Whether the list should keep being re-read: the control plane discovers in
 * the background after an install, and a row with no attempt yet has not been
 * reached. Off as soon as every row has an attempt, whatever its outcome.
 */
export function discoveryOutstanding(
  servers: readonly Pick<McpServerRow, "tools_discovery_attempted_at">[] | undefined,
): boolean {
  return !!servers?.some((server) => server.tools_discovery_attempted_at === null);
}

/** react-query `refetchInterval` for the server list. */
export function discoveryRefetchInterval(
  servers: readonly Pick<McpServerRow, "tools_discovery_attempted_at">[] | undefined,
): number | false {
  return discoveryOutstanding(servers) ? DISCOVERY_POLL_MS : false;
}
