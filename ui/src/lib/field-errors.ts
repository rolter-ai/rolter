import { ApiError } from "@/lib/api";

/**
 * Pins a rejected save to the field it names (#2096).
 *
 * The control plane answers a bad value with a 400 whose message opens with the
 * wire name of the field (`retention_days must be between 1 and 3650`). The
 * screen hands in its wire-name → form-key table and gets back the field to
 * mark, or null when the rejection names none of them and belongs in a toast.
 */
export function serverFieldError<K extends string>(
  error: unknown,
  fields: Record<string, K>,
): { field: K; message: string } | null {
  if (!(error instanceof ApiError) || error.status !== 400) return null;
  const wire = /^([a-z][a-z0-9_]*)\b/.exec(error.message)?.[1];
  const field = wire ? fields[wire] : undefined;
  return field ? { field, message: error.message } : null;
}
