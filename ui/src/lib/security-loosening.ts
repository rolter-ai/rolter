/**
 * Which part of a Security save takes a protection away.
 *
 * The screen saves every field in one request, so a reviewer cannot tell from
 * the button whether it tightens the deployment or opens it. One edit opens
 * something, and only that raises the confirmation: **a bypass route added.**
 * `auth_bypass_routes` gains a path, which then answers with no key at all.
 *
 * The screen once had an "enforce virtual keys" switch whose off position
 * counted here too. It was removed (#2357): it only reached managed gateways,
 * which refuse a keyless request whatever it said.
 *
 * Every other edit, including a route taken out or a header required, leaves
 * the deployment as closed as it was or closes it further. A dialog in front
 * of those is the click-through that teaches people to dismiss the one that
 * matters, so they save at once.
 */

/** The settings a loosening can come from. */
export interface SecurityPolicy {
  authBypassRoutes: readonly string[];
}

export type Loosening = { kind: "bypassRoute"; route: string };

/**
 * The protections `next` removes from `saved`: each route added, in the order
 * the screen lists them.
 *
 * `saved` is what the control plane holds now, not the draft. A route that was
 * already exempt and stays so is not a new one. Routes compare exactly, as the
 * gateway matches them.
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
  return out;
}
