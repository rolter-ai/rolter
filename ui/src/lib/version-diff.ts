import type { PromptTemplateDecorator, PromptTemplateVariable, SkillVersionRow } from "@/lib/api";
import {
  countChanges,
  diffFields,
  diffLines,
  type DiffLine,
  type FieldChange,
} from "@/lib/text-diff";

// what changed between two versions of a prompt template or a skill, the
// comparison the version rail, the make-live dialog and the compare dialog all
// read from (#2112). pure, so the stories and unit tests can state it without a
// screen

const changedFields = (fields: FieldChange[]) =>
  fields.filter((field) => field.kind !== "same").length;

/** the decorators as the text a reader compares: a header line per message, then its content */
export function decoratorText(
  decorators: PromptTemplateDecorator[],
  header: (decorator: PromptTemplateDecorator) => string = (decorator) =>
    `${decorator.role} · ${decorator.position}`,
): string {
  return decorators.map((decorator) => `[${header(decorator)}]\n${decorator.content}`).join("\n\n");
}

export interface PromptDiff {
  /** one row per variable, by name */
  variables: FieldChange[];
  decorators: DiffLine[];
  /** variables changed plus decorator lines added or removed; 0 means the two are the same */
  changed: number;
}

/**
 * `to` against `from`. `describe` writes a variable's mode as the words the
 * screen uses ("required", "default: x"), so a rename of a default reads as one
 * changed row.
 */
export function diffPromptVersions(
  from: { variables: PromptTemplateVariable[]; decorators: PromptTemplateDecorator[] },
  to: { variables: PromptTemplateVariable[]; decorators: PromptTemplateDecorator[] },
  describe: (variable: PromptTemplateVariable) => string,
  header?: (decorator: PromptTemplateDecorator) => string,
): PromptDiff {
  const byName = (variables: PromptTemplateVariable[]) =>
    Object.fromEntries(variables.map((variable) => [variable.name, describe(variable)]));
  const variables = diffFields(byName(from.variables), byName(to.variables));
  const decorators = diffLines(
    decoratorText(from.decorators, header),
    decoratorText(to.decorators, header),
  );
  const { added, removed } = countChanges(decorators);
  return { variables, decorators, changed: changedFields(variables) + added + removed };
}

export interface SkillDiff {
  /** the source mode and, for an artifact reference, the reference itself */
  source: FieldChange[];
  /** the inline content; empty on both sides for a pair of references */
  content: DiffLine[];
  /** one row per top-level metadata key */
  metadata: FieldChange[];
  changed: number;
}

/** `to` against `from`; `labels` name the two source rows */
export function diffSkillVersions(
  from: SkillVersionRow,
  to: SkillVersionRow,
  labels: { source: string; reference: string; inline: string; artifact: string },
): SkillDiff {
  const mode = (version: SkillVersionRow) =>
    version.content_ref ? labels.artifact : labels.inline;
  const source: FieldChange[] = diffFields(
    {
      [labels.source]: mode(from),
      ...(from.content_ref ? { [labels.reference]: from.content_ref } : {}),
    },
    {
      [labels.source]: mode(to),
      ...(to.content_ref ? { [labels.reference]: to.content_ref } : {}),
    },
  );
  const content = diffLines(from.content ?? "", to.content ?? "");
  const metadata = diffFields(flatten(from.metadata), flatten(to.metadata));
  const { added, removed } = countChanges(content);
  return {
    source,
    content,
    metadata,
    changed: changedFields(source) + changedFields(metadata) + added + removed,
  };
}

/** each top-level key's value as compact JSON, so a nested object is one comparable cell */
function flatten(metadata: Record<string, unknown>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(metadata ?? {}).map(([key, value]) => [key, JSON.stringify(value)]),
  );
}
