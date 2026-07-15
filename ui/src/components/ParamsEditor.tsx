import { AlertTriangle, Lock, LockOpen, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { InfoHint } from "@/components/ui/info-hint";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";

// known optional sampling/inference params across OpenAI and Anthropic; used
// only for a soft warning since keys are provider-agnostic and callers may
// legitimately pass provider-specific extras
const KNOWN_PARAMS = new Set([
  "temperature",
  "top_p",
  "top_k",
  "min_p",
  "max_tokens",
  "max_completion_tokens",
  "presence_penalty",
  "frequency_penalty",
  "repetition_penalty",
  "stop",
  "stop_sequences",
  "seed",
  "n",
  "logprobs",
  "top_logprobs",
  "logit_bias",
  "response_format",
  "tools",
  "tool_choice",
  "parallel_tool_calls",
  "stream",
  "user",
  "metadata",
  "system",
  "thinking",
  "reasoning_effort",
  "service_tier",
  "verbosity",
]);

type ParamType = "string" | "number" | "boolean" | "json";

interface ParamRow {
  id: number;
  key: string;
  type: ParamType;
  // canonical text representation of the value for the row's type
  value: string;
  // whether callers are barred from overriding this param; only consulted in
  // "manual" policy mode
  locked: boolean;
}

// the three admin-facing override policies. the backend models only allow/deny
// with per-param exception lists; "manual" is the UI affordance for mixed
// per-param control, serialized as allow-mode + a deny list of the locked keys
type PolicyMode = "allow" | "deny" | "manual";

let rowSeq = 0;
const nextId = () => ++rowSeq;

function inferType(value: unknown): ParamType {
  if (typeof value === "number") return "number";
  if (typeof value === "boolean") return "boolean";
  if (typeof value === "string") return "string";
  return "json";
}

function toRowValue(value: unknown, type: ParamType): string {
  if (type === "boolean") return value ? "true" : "false";
  if (type === "string") return String(value ?? "");
  if (type === "number") return value === undefined ? "" : String(value);
  return JSON.stringify(value ?? null);
}

function readList(policy: Record<string, unknown>, field: "allow" | "deny"): string[] {
  const v = policy[field];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

// resolve the stored policy into a UI mode plus, for manual, the set of locked
// param keys. a bare allow/deny with no exceptions maps to the simple modes;
// anything with exceptions is manual mixed control
function readPolicy(policy: Record<string, unknown>): {
  mode: PolicyMode;
  locked: Set<string>;
} {
  const base = policy.mode === "deny" ? "deny" : "allow";
  const allow = readList(policy, "allow");
  const deny = readList(policy, "deny");
  if (base === "allow" && deny.length === 0) return { mode: "allow", locked: new Set() };
  if (base === "deny" && allow.length === 0) return { mode: "deny", locked: new Set() };
  // manual: under allow-base the deny list is locked; under deny-base every
  // default not explicitly allowed is locked (resolved per-row at load time)
  return { mode: "manual", locked: new Set(base === "allow" ? deny : []) };
}

function rowsFromParams(
  params: Record<string, unknown>,
  policy: Record<string, unknown>,
): { rows: ParamRow[]; mode: PolicyMode } {
  const { mode, locked } = readPolicy(policy);
  const base = policy.mode === "deny" ? "deny" : "allow";
  const allow = readList(policy, "allow");
  const rows = Object.entries(params).map(([key, value]) => {
    const type = inferType(value);
    // under a deny-base manual policy, a param is locked unless it's allowed
    const isLocked =
      mode === "manual" && base === "deny" ? !allow.includes(key) : locked.has(key);
    return { id: nextId(), key, type, value: toRowValue(value, type), locked: isLocked };
  });
  return { rows, mode };
}

// serialize rows back to a params object; throws with a human message on the
// first row that fails to parse for its declared type
function rowsToParams(rows: ParamRow[]): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (!key) continue;
    if (key in out) throw new Error(`duplicate param "${key}"`);
    switch (row.type) {
      case "string":
        out[key] = row.value;
        break;
      case "number": {
        const n = Number(row.value);
        if (row.value.trim() === "" || Number.isNaN(n)) {
          throw new Error(`"${key}": not a valid number`);
        }
        out[key] = n;
        break;
      }
      case "boolean":
        out[key] = row.value === "true";
        break;
      case "json":
        try {
          out[key] = JSON.parse(row.value || "null");
        } catch {
          throw new Error(`"${key}": invalid JSON value`);
        }
        break;
    }
  }
  return out;
}

function policyFor(mode: PolicyMode, rows: ParamRow[]): Record<string, unknown> {
  if (mode === "allow") return { mode: "allow", allow: [], deny: [] };
  if (mode === "deny") return { mode: "deny", allow: [], deny: [] };
  // manual: allow-base with the locked keys denied
  const deny = rows.filter((r) => r.locked && r.key.trim()).map((r) => r.key.trim());
  return { mode: "allow", allow: [], deny };
}

export interface ParamsEditorValue {
  params: Record<string, unknown>;
  paramPolicy: Record<string, unknown>;
}

export type ParamsEditorResult =
  | { ok: true; value: ParamsEditorValue }
  | { ok: false; error: string };

interface CommonProps {
  params?: Record<string, unknown>;
  paramPolicy?: Record<string, unknown>;
}

interface EditProps extends CommonProps {
  variant?: "edit";
  saving: boolean;
  error?: string | null;
  onSave: (value: ParamsEditorValue) => void;
}

interface CreateProps extends CommonProps {
  variant: "create";
  onChange: (result: ParamsEditorResult) => void;
}

/**
 * structured editor for a route's admin default inference params and the
 * override policy that governs whether callers may replace them. typed
 * key/value rows plus a three-way policy: callers may override everything,
 * nothing, or a per-param mix ("manual") toggled inline per row. surfaces a
 * soft warning for unrecognized param keys.
 *
 * `variant="edit"` renders its own Save button and calls `onSave`;
 * `variant="create"` is controlled — it reports the serialized value (or a
 * validation error) up via `onChange` so the parent can persist it after the
 * route is created.
 */
export function ParamsEditor(props: EditProps | CreateProps) {
  const isCreate = props.variant === "create";
  const params = props.params;
  const paramPolicy = props.paramPolicy;

  const [rows, setRows] = React.useState<ParamRow[]>([]);
  const [mode, setMode] = React.useState<PolicyMode>("allow");
  const [localError, setLocalError] = React.useState<string | null>(null);

  React.useEffect(() => {
    const seeded = rowsFromParams(params ?? {}, paramPolicy ?? {});
    setRows(seeded.rows);
    setMode(seeded.mode);
    setLocalError(null);
  }, [params, paramPolicy]);

  // controlled create variant: report serialized value / error on every change
  const onChange = isCreate ? props.onChange : undefined;
  React.useEffect(() => {
    if (!onChange) return;
    try {
      const value = { params: rowsToParams(rows), paramPolicy: policyFor(mode, rows) };
      onChange({ ok: true, value });
    } catch (e) {
      onChange({ ok: false, error: (e as Error).message });
    }
  }, [rows, mode, onChange]);

  const updateRow = (id: number, patch: Partial<ParamRow>) =>
    setRows((rs) => rs.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  const addRow = () =>
    setRows((rs) => [
      ...rs,
      { id: nextId(), key: "", type: "string", value: "", locked: false },
    ]);

  const removeRow = (id: number) => setRows((rs) => rs.filter((r) => r.id !== id));

  const unknownKeys = rows
    .map((r) => r.key.trim())
    .filter((k) => k && !KNOWN_PARAMS.has(k));

  const submit = () => {
    if (isCreate) return;
    let params: Record<string, unknown>;
    try {
      params = rowsToParams(rows);
    } catch (e) {
      setLocalError((e as Error).message);
      return;
    }
    setLocalError(null);
    props.onSave({ params, paramPolicy: policyFor(mode, rows) });
  };

  const MODE_LABELS: Record<PolicyMode, string> = {
    allow: "Callers may override",
    deny: "Locked by default",
    manual: "Manual",
  };
  const MODE_HINTS: Record<PolicyMode, string> = {
    allow: "Callers can override any default above.",
    deny: "Callers can't override any default — the admin values always win.",
    manual: "Set override or lock per param with the toggle on each row.",
  };

  return (
    <div className="space-y-3 rounded-md border border-dashed border-border p-3">
      <div className="space-y-0.5">
        <div className="flex items-center gap-1.5">
          <p className="text-sm font-medium leading-none">Params</p>
          <InfoHint
            label="About params"
            text="Default inference params sent to the upstream on every request for this model (e.g. temperature 0.7, max_tokens 1024). Pick the value's type per row; json accepts arrays/objects like stop sequences."
          />
        </div>
        <p className="text-xs text-muted-foreground">
          Admin default inference params, applied reload-free on save.
        </p>
      </div>

      <div className="space-y-1.5">
        {rows.length === 0 && (
          <p className="text-xs text-muted-foreground">No default params.</p>
        )}
        {rows.map((row) => {
          const unknown = row.key.trim() !== "" && !KNOWN_PARAMS.has(row.key.trim());
          return (
            <div key={row.id} className="flex items-start gap-1.5">
              <div className="relative flex-1">
                <Input
                  aria-label="Param name"
                  className="h-8 font-mono text-xs"
                  placeholder="temperature"
                  value={row.key}
                  onChange={(e) => updateRow(row.id, { key: e.target.value })}
                  list="known-params"
                />
                {unknown && (
                  <AlertTriangle
                    aria-label="Unrecognized param key"
                    className="pointer-events-none absolute right-2 top-2 h-3.5 w-3.5 text-amber-500"
                  />
                )}
              </div>
              <Select
                aria-label="Param type"
                className="h-8 w-24 text-xs"
                value={row.type}
                onChange={(e) => {
                  const type = e.target.value as ParamType;
                  // reset the value to a sane default for the new type
                  const value =
                    type === "boolean" ? "false" : type === "json" ? "null" : "";
                  updateRow(row.id, { type, value });
                }}
              >
                <option value="string">string</option>
                <option value="number">number</option>
                <option value="boolean">bool</option>
                <option value="json">json</option>
              </Select>
              {row.type === "boolean" ? (
                <Select
                  aria-label="Param value"
                  className="h-8 flex-1 text-xs"
                  value={row.value}
                  onChange={(e) => updateRow(row.id, { value: e.target.value })}
                >
                  <option value="true">true</option>
                  <option value="false">false</option>
                </Select>
              ) : (
                <Input
                  aria-label="Param value"
                  className="h-8 flex-1 font-mono text-xs"
                  type={row.type === "number" ? "number" : "text"}
                  placeholder={row.type === "json" ? '["\\n"]' : "0"}
                  value={row.value}
                  onChange={(e) => updateRow(row.id, { value: e.target.value })}
                />
              )}
              {mode === "manual" && (
                <button
                  type="button"
                  aria-label={
                    row.locked
                      ? "Locked — callers can't override; click to allow"
                      : "Overridable — callers may override; click to lock"
                  }
                  aria-pressed={row.locked}
                  onClick={() => updateRow(row.id, { locked: !row.locked })}
                  className={
                    "mt-0.5 shrink-0 rounded-md border border-input p-1.5 transition-colors hover:bg-transparent " +
                    (row.locked
                      ? "bg-destructive text-destructive-foreground hover:text-destructive"
                      : "bg-[color:var(--surface-subtle)] text-muted-foreground hover:text-foreground")
                  }
                >
                  {row.locked ? (
                    <Lock className="h-3.5 w-3.5" />
                  ) : (
                    <LockOpen className="h-3.5 w-3.5" />
                  )}
                </button>
              )}
              <button
                type="button"
                aria-label="Remove param"
                onClick={() => removeRow(row.id)}
                className="mt-1.5 shrink-0 text-muted-foreground hover:text-destructive"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          );
        })}
        <datalist id="known-params">
          {[...KNOWN_PARAMS].map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
        <Button size="sm" variant="ghost" className="h-7 px-2" onClick={addRow}>
          <Plus className="h-3.5 w-3.5" />
          Add param
        </Button>
      </div>

      {unknownKeys.length > 0 && (
        <p className="flex items-start gap-1.5 text-xs text-amber-600 dark:text-amber-500">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            Not a standard OpenAI/Anthropic param: {unknownKeys.join(", ")}. Saved
            as-is — check the spelling if unintended.
          </span>
        </p>
      )}

      <div className="space-y-2 border-t border-border pt-3">
        <div className="flex items-center gap-1.5">
          <p className="text-sm font-medium leading-none">Override policy</p>
          <InfoHint
            label="About override policy"
            text="Governs whether a caller's request params can replace these admin defaults. 'Callers may override' allows all; 'Locked by default' allows none; 'Manual' decides per param with the lock toggle on each row."
          />
        </div>
        <div className="inline-flex rounded-md border border-border p-0.5 text-xs">
          {(["allow", "deny", "manual"] as const).map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => setMode(m)}
              className={
                "rounded px-2.5 py-1 font-medium transition-colors " +
                (mode === m
                  ? "bg-foreground text-background"
                  : "text-muted-foreground hover:text-foreground")
              }
            >
              {MODE_LABELS[m]}
            </button>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">{MODE_HINTS[mode]}</p>
      </div>

      {(localError || (!isCreate && props.error)) && (
        <p className="text-xs text-destructive">
          {localError || (!isCreate ? props.error : null)}
        </p>
      )}
      {!isCreate && (
        <Button size="sm" variant="outline" disabled={props.saving} onClick={submit}>
          Save params
        </Button>
      )}
    </div>
  );
}
