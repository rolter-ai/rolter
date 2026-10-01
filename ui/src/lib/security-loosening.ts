/**
 * Which part of a Security save takes a protection away.
 *
 * The screen saves every field in one request, so a reviewer cannot tell from
 * the button whether it tightens the deployment or opens it. Two edits open
 * something, and only those raise the confirmation:
 *
 * - **Virtual keys not enforced.** `virtual_key_required` goes from on to off.
 *   A gateway that holds no virtual keys stops refusing every request by
 *   default, and is left to decide by how it was started.
 * - **A bypass route added.** `auth_bypass_routes` gains a path, which then
 *   answers with no key at all.
 *
 * Every other edit, including a route taken out or a header required, leaves
 * the deployment as closed as it was or closes it further. A dialog in front
 * of those is the click-through that teaches people to dismiss the one that
 * matters, so they save at once.
 */

/** The settings a loosening can come from. */
export interface SecurityPolicy {
  virtualKeyRequired: boolean;
  authBypassRoutes: readonly string[];
}

export type Loosening = { kind: "virtualKeys" } | { kind: "bypassRoute"; route: string };

/**
 * The protections `next` removes from `saved`, in the order the screen lists
 * them: virtual keys, then each route added.
 *
 * `saved` is what the control plane holds now, not the draft. A switch flipped
 * off and back on is no change, and a route that was already exempt and stays
 * so is not a new one. Routes compare exactly, as the gateway matches them.
 */
export function loosenings(saved: SecurityPolicy, next: SecurityPolicy): Loosening[] {
  const out: Loosening[] = [];
  if (saved.virtualKeyRequired && !next.virtualKeyRequired) out.push({ kind: "virtualKeys" });
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
