/**
 * Which part of a Security save takes a protection away.
 *
 * The screen saves every field in one request, so a reviewer cannot tell from
 * the button whether it tightens the deployment or opens it. Two edits open
 * something, and only those raise the confirmation: **a bypass route added**
 * (`auth_bypass_routes` gains a path, which then answers with no key at all),
 * and **a required header removed** (`required_headers` loses a name, so the
 * ingress filter it stood for, a mesh identity or a WAF marker, stops applying
 * and requests without it are served).
 *
 * The screen once had an "enforce virtual keys" switch whose off position
 * counted here too. It was removed (#2357): it only reached managed gateways,
 * which refuse a keyless request whatever it said.
 *
 * Every other edit, including a route taken out, a header required or a
 * required header's value changed, leaves the deployment as closed as it was or
 * closes it further, so it saves at once. **An allowed origin added** opens
 * something too, and is deliberately not here (#2358): it is the routine edit
 * of this screen, and a dialog on every origin is the click-through that
 * teaches people to dismiss the one that matters. See
 * `docs/dev-docs/development/destructive-actions.md`.
 */

/** The settings a loosening can come from. */
export interface SecurityPolicy {
  authBypassRoutes: readonly string[];
  /** the names of the required headers; a value is not a protection of its own */
  requiredHeaders: readonly string[];
}

export type Loosening =
  { kind: "bypassRoute"; route: string } | { kind: "requiredHeader"; name: string };

/** What a loosening names, for the row that lists it. */
export const looseningSubject = (item: Loosening): string =>
  item.kind === "bypassRoute" ? item.route : item.name;

/**
 * The protections `next` removes from `saved`: each route added, then each
 * required header removed, in the order the screen lists them.
 *
 * `saved` is what the control plane holds now, not the draft. A route that was
 * already exempt and stays so is not a new one. Routes compare exactly, as the
 * gateway matches them; header names compare without regard to case, as the
 * gateway does, so retyping `X-Tenant` as `x-tenant` removes nothing.
 */
export function loosenings(saved: SecurityPolicy, next: SecurityPolicy): Loosening[] {
  const out: Loosening[] = [];
  const had = new Set(saved.authBypassRoutes);
  const added = new Set<string>();
  for (const route of next.authBypassRoutes) {
    // a route typed twice is one new exemption, not two
    if (had.has(route) || added.has(route)) continue;
    added.add(route);
    out.push({ kind: "bypassRoute", route });
  }
  const kept = new Set(next.requiredHeaders.map((name) => name.toLowerCase()));
  const removed = new Set<string>();
  for (const name of saved.requiredHeaders) {
    const lower = name.toLowerCase();
    if (kept.has(lower) || removed.has(lower)) continue;
    removed.add(lower);
    out.push({ kind: "requiredHeader", name });
  }
  return out;
}
