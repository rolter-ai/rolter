import { ApiError } from "@/lib/api";

/**
 * The request field a rejected save names, from the control plane's structured
 * `error.field` (#2735). A path into a list or map (`forwarded_headers[2]`,
 * `injected_headers.x-team`) keeps its row for a screen that can place it; the
 * `root` is the part before the first `.` or `[`, which a screen with one input
 * for the whole list can use.
 */
export function serverFieldPath(error: unknown): { path: string; root: string } | null {
  if (!(error instanceof ApiError) || error.status !== 400 || !error.field) return null;
  return { path: error.field, root: /^[^.[]+/.exec(error.field)?.[0] ?? error.field };
}

/**
 * Pins a rejected save to the field it names (#2096).
 *
 * The control plane answers a bad value with a 400 that carries the wire name
 * of the field as `error.field`; the screen hands in its wire-name → form-key
 * table and gets back the field to mark, or null when the rejection names none
 * of them and belongs in a toast. An older control plane, or a refusal that
 * sends no `field`, is matched by the wire name its message opens with
 * (`retention_days must be between 1 and 3650`).
 */
export function serverFieldError<K extends string>(
  error: unknown,
  fields: Record<string, K>,
): { field: K; message: string } | null {
  if (!(error instanceof ApiError) || error.status !== 400) return null;
  const named = serverFieldPath(error);
  const structured = named ? (fields[named.path] ?? fields[named.root]) : undefined;
  if (structured) return { field: structured, message: error.message };
  const wire = /^([a-z][a-z0-9_]*)\b/.exec(error.message)?.[1];
  const field = wire ? fields[wire] : undefined;
  return field ? { field, message: error.message } : null;
}
