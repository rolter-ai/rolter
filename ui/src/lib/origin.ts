/**
 * Whether saving `next` over `stored` points an endpoint at another scheme,
 * host or port.
 *
 * A bearer secret belongs to the receiver it was given for, so a form that
 * edits an endpoint says what becomes of the stored one when the origin moves.
 * An endpoint that does not parse moves nothing here, because the control plane
 * refuses it anyway.
 */
export function movesOrigin(stored: string, next: string): boolean {
  try {
    return new URL(stored).origin !== new URL(next.trim()).origin;
  } catch {
    return false;
  }
}
