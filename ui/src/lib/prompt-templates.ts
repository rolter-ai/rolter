import type {
  PromptTemplateDecorator,
  PromptTemplateScopeInput,
  PromptTemplateVariable,
} from "@/lib/api";

/**
 * The gateway's variable rules for prompt templates, mirrored so the dashboard
 * can say who a publish will refuse before it does (#2110).
 *
 * They live in `crates/rolter-core/src/prompt_templates.rs`:
 *
 * - a `{{ name }}` placeholder resolves from the caller's
 *   `rolter_template_vars`, then from the variable's default, and the request is
 *   refused with 400 `invalid_prompt_template` when neither has a value. The
 *   `required` flag is never read on the request path, so a variable a caller
 *   must send is one a decorator references and that has no default
 * - a caller variable that no template active for the request declares is
 *   refused the same way, so a version that drops a variable breaks every
 *   caller still sending it
 * - every template whose scopes match the request applies; none outranks
 *   another, so a version's scopes are exactly the requests it reaches
 */

export interface TemplateContent {
  variables: PromptTemplateVariable[];
  decorators: PromptTemplateDecorator[];
}

const PLACEHOLDER = /{{\s*([A-Za-z_][A-Za-z0-9_]*)\s*}}/g;

/** the variable names the decorators reference, in first-use order */
export function referencedVariables(decorators: PromptTemplateDecorator[]): string[] {
  const names = new Set<string>();
  for (const decorator of decorators) {
    for (const match of decorator.content.matchAll(PLACEHOLDER)) names.add(match[1]);
  }
  return [...names];
}

/**
 * The variables a request must carry for this content to render, in declared
 * order. `default` is read as the control plane stores it: absent (or null) is
 * no default, and an empty string is a default like any other.
 */
export function requiredVariables(content: TemplateContent): string[] {
  const referenced = new Set(referencedVariables(content.decorators));
  return content.variables
    .filter((variable) => referenced.has(variable.name) && variable.default == null)
    .map((variable) => variable.name);
}

export interface PublishImpact {
  /** every variable a request in scope must send once `target` is live */
  required: string[];
  /** of `required`, the ones the live version did not need */
  newlyRequired: string[];
  /** declared by the live version and not by `target`: a caller still sending one is refused */
  dropped: string[];
}

/** What making `target` live changes for callers, against the version live now. */
export function publishImpact(target: TemplateContent, live?: TemplateContent): PublishImpact {
  const required = requiredVariables(target);
  if (!live) return { required, newlyRequired: [], dropped: [] };
  const liveRequired = new Set(requiredVariables(live));
  const declared = new Set(target.variables.map((variable) => variable.name));
  return {
    required,
    newlyRequired: required.filter((name) => !liveRequired.has(name)),
    dropped: live.variables.map((variable) => variable.name).filter((name) => !declared.has(name)),
  };
}

export function scopeKey(scope: PromptTemplateScopeInput): string {
  return `${scope.scope_type}:${scope.scope_id}`;
}

/**
 * The scopes `target` reaches that the live version does not: the requests
 * there have never been decorated by this template, so they send none of its
 * variables yet. With no live version, or none known, every scope is new.
 */
export function addedScopes(
  target: PromptTemplateScopeInput[],
  live: PromptTemplateScopeInput[] | undefined,
): Set<string> {
  const before = new Set((live ?? []).map(scopeKey));
  return new Set(target.map(scopeKey).filter((key) => !before.has(key)));
}

/**
 * The `rolter_template_vars` fragment of a request body for this content, as a
 * caller would paste it: every declared variable, valued with the sample typed
 * into the preview, else its default, else `blank`. Only the fragment, because
 * the rest of the body is whatever the caller already sends.
 */
export function templateVarsExample(
  variables: PromptTemplateVariable[],
  samples: Record<string, string>,
  blank = "…",
): string {
  const values: Record<string, string> = {};
  for (const variable of variables) {
    if (!variable.name) continue;
    values[variable.name] = samples[variable.name] || variable.default || blank;
  }
  return JSON.stringify({ rolter_template_vars: values }, null, 2);
}
