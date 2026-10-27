// prompt template repository (crates/rolter-control/src/crud/prompts.rs)
import { getJson, sendJson } from "./request";

export interface PromptTemplateVariable {
  name: string;
  required: boolean;
  default?: string;
}

export type PromptDecoratorRole = "system" | "assistant" | "user";

export type PromptDecoratorPosition = "prepend" | "append";

export interface PromptTemplateDecorator {
  role: PromptDecoratorRole;
  position: PromptDecoratorPosition;
  content: string;
}

export interface PromptTemplateRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description?: string | null;
  published_version?: number | null;
  created_at: string;
}

export interface PromptTemplateVersionRow {
  template_id: string;
  version: number;
  variables: PromptTemplateVariable[];
  decorators: PromptTemplateDecorator[];
  created_at: string;
}

export type PromptTemplateScopeType = "org" | "project" | "route" | "virtual_key";

export interface PromptTemplateScopeRow {
  template_id: string;
  version: number;
  scope_type: PromptTemplateScopeType;
  scope_id: string;
  created_at: string;
}

export interface PromptTemplateScopeInput {
  scope_type: PromptTemplateScopeType;
  scope_id: string;
}

export function fetchPromptTemplates(orgId: string): Promise<PromptTemplateRow[]> {
  return getJson<PromptTemplateRow[]>(`/api/v1/orgs/${orgId}/prompt-templates`);
}

export function createPromptTemplate(
  orgId: string,
  input: { name: string; slug?: string; description?: string },
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>("POST", `/api/v1/orgs/${orgId}/prompt-templates`, input);
}

export function updatePromptTemplate(
  templateId: string,
  input: { name?: string; description?: string },
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>("PUT", `/api/v1/prompt-templates/${templateId}`, input);
}

export function deletePromptTemplate(templateId: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/prompt-templates/${templateId}`);
}

export function fetchPromptTemplateVersions(
  templateId: string,
): Promise<PromptTemplateVersionRow[]> {
  return getJson<PromptTemplateVersionRow[]>(`/api/v1/prompt-templates/${templateId}/versions`);
}

export function createPromptTemplateVersion(
  templateId: string,
  input: {
    variables: PromptTemplateVariable[];
    decorators: PromptTemplateDecorator[];
  },
): Promise<PromptTemplateVersionRow> {
  return sendJson<PromptTemplateVersionRow>(
    "POST",
    `/api/v1/prompt-templates/${templateId}/versions`,
    input,
  );
}

export function fetchPromptTemplateScopes(
  templateId: string,
  version: number,
): Promise<PromptTemplateScopeRow[]> {
  return getJson<PromptTemplateScopeRow[]>(
    `/api/v1/prompt-templates/${templateId}/versions/${version}/scopes`,
  );
}

export function setPromptTemplateScopes(
  templateId: string,
  version: number,
  scopes: PromptTemplateScopeInput[],
): Promise<PromptTemplateScopeRow[]> {
  return sendJson<PromptTemplateScopeRow[]>(
    "PUT",
    `/api/v1/prompt-templates/${templateId}/versions/${version}/scopes`,
    { scopes },
  );
}

export function publishPromptTemplateVersion(
  templateId: string,
  version: number,
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>("PUT", `/api/v1/prompt-templates/${templateId}/publish`, {
    version,
  });
}

export function rollbackPromptTemplateVersion(
  templateId: string,
  version: number,
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>("PUT", `/api/v1/prompt-templates/${templateId}/rollback`, {
    version,
  });
}
