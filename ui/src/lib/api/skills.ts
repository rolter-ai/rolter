// organization skills repository (crates/rolter-control/src/crud/skills.rs)
import { getJson, sendJson } from "./request";

export type SkillMinimumRole = "viewer" | "member" | "admin";

export interface SkillRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description: string;
  retired_at?: string | null;
  published_version?: number | null;
  allowed_team_ids: string[];
  minimum_role: SkillMinimumRole;
  created_at: string;
}

export interface SkillVersionRow {
  skill_id: string;
  version: number;
  content?: string | null;
  content_ref?: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface CreateSkillInput {
  name: string;
  slug?: string;
  description?: string;
  allowed_team_ids?: string[];
  minimum_role?: SkillMinimumRole;
}

export interface UpdateSkillInput {
  name?: string;
  description?: string;
  retired?: boolean;
  allowed_team_ids?: string[];
  minimum_role?: SkillMinimumRole;
}

export function fetchSkills(orgId: string): Promise<SkillRow[]> {
  return getJson<SkillRow[]>(`/api/v1/orgs/${orgId}/skills`);
}

export function createSkill(orgId: string, input: CreateSkillInput): Promise<SkillRow> {
  return sendJson<SkillRow>("POST", `/api/v1/orgs/${orgId}/skills`, input);
}

export function updateSkill(id: string, input: UpdateSkillInput): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}`, input);
}

export function deleteSkill(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/skills/${id}`);
}

export function fetchSkillVersions(id: string): Promise<SkillVersionRow[]> {
  return getJson<SkillVersionRow[]>(`/api/v1/skills/${id}/versions`);
}

export function createSkillVersion(
  id: string,
  input:
    | {
        content: string;
        content_ref?: never;
        metadata: Record<string, unknown>;
      }
    | {
        content?: never;
        content_ref: string;
        metadata: Record<string, unknown>;
      },
): Promise<SkillVersionRow> {
  return sendJson<SkillVersionRow>("POST", `/api/v1/skills/${id}/versions`, input);
}

export function publishSkillVersion(id: string, version: number): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}/publish`, { version });
}

export function rollbackSkillVersion(id: string, version: number): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}/rollback`, {
    version,
  });
}
