import { ArrowUpRight, Folder, Trash2, User } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Pill } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Overline } from "@/components/ui/overline";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import type { MembershipRow, OrgProjectRow, TeamRow, UserRow } from "@/lib/api";
import { RowCapabilityScope } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { teamPeople } from "@/lib/team-people";

/** What a read the sheet depends on is doing: still going, failed, or answered. */
interface Read {
  isLoading: boolean;
  error: unknown;
  refetch: () => void;
}

/**
 * One team, opened from its card: its projects, and who holds which role on the
 * team and on each project (#2072).
 *
 * The card counts and names; this lists. It reads nothing itself, because the
 * card already holds the org's memberships, accounts and projects and a second
 * copy of each query would only be a second place for them to disagree. Its
 * controls are gated at the team's own chain (`RowCapabilityScope`), not at
 * whatever scope the sidebar happens to hold, so a team admin is offered their
 * own team's actions and nothing of another team's.
 *
 * There is no rename here: the control plane has no endpoint to change a team's
 * name, and a control that sends nothing is worse than none.
 */
export function TeamSheet({
  team,
  projects,
  memberships,
  users,
  onClose,
  onDelete,
}: {
  team: TeamRow;
  /** every project in the org; the sheet keeps this team's */
  projects: Read & { data: OrgProjectRow[] | undefined };
  memberships: Read & { data: MembershipRow[] | undefined };
  users: UserRow[] | undefined;
  onClose: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();

  const own = projects.data?.filter((p) => p.team_id === team.id);
  const people = memberships.data
    ? teamPeople(
        memberships.data,
        team.id,
        (own ?? []).map((p) => p.id),
      )
    : undefined;

  // an account this caller cannot resolve is drawn by its id rather than hidden:
  // the grant is real whether or not its owner's name could be read
  const nameOf = (id: string) => {
    const user = users?.find((u) => u.id === id);
    return user ? user.display_name || user.email : id;
  };

  const grantRow = (grant: MembershipRow) => (
    <li key={grant.id} className="flex items-center gap-2 text-sm">
      <User className="size-3.5 flex-none text-muted-foreground" aria-hidden />
      <span className="min-w-0 flex-1 truncate" title={nameOf(grant.user_id)}>
        {nameOf(grant.user_id)}
      </span>
      <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
        {t(`shell.roles.${grant.role}`, { defaultValue: grant.role })}
      </Pill>
    </li>
  );

  return (
    <RowCapabilityScope at={{ projectId: null, teamId: team.id }}>
      <Sheet open onOpenChange={(open) => !open && onClose()}>
        <SheetHeader
          title={team.name}
          subtitle={t("pages.govTeams.created", { date: fmt.date(team.created_at ?? "") })}
          onClose={onClose}
        />
        <SheetBody>
          <section className="space-y-2" aria-labelledby={`team-${team.id}-people`}>
            <Overline as="h3" id={`team-${team.id}-people`}>
              {t("pages.teams.sheet.people")}
            </Overline>
            {memberships.isLoading && <ListSkeleton rows={2} />}
            {memberships.error ? (
              <LoadError
                error={memberships.error}
                resource={t("errors.resources.memberships")}
                onRetry={memberships.refetch}
                target="team-people"
              />
            ) : null}
            {people && people.teamGrants.length === 0 && (
              <EmptyState
                inline
                uxTarget="team-people"
                title={t("pages.teams.sheet.noPeople")}
                description={t("pages.teams.sheet.noPeople")}
              />
            )}
            {people && people.teamGrants.length > 0 && (
              <ul aria-label={t("pages.teams.sheet.people")} className="space-y-1.5">
                {people.teamGrants.map(grantRow)}
              </ul>
            )}
          </section>

          <section className="space-y-2" aria-labelledby={`team-${team.id}-projects`}>
            <Overline as="h3" id={`team-${team.id}-projects`}>
              {t("pages.teams.sheet.projects")}
            </Overline>
            {projects.isLoading && <ListSkeleton rows={2} />}
            {projects.error ? (
              <LoadError
                error={projects.error}
                resource={t("errors.resources.projects")}
                onRetry={projects.refetch}
                target="team-projects"
              />
            ) : null}
            {own && own.length === 0 && (
              <EmptyState
                inline
                uxTarget="team-projects"
                title={t("pages.teams.sheet.noProjects")}
                description={t("pages.teams.sheet.noProjects")}
              />
            )}
            {own && own.length > 0 && (
              <ul aria-label={t("pages.teams.sheet.projects")} className="space-y-3">
                {own.map((project) => {
                  const grants = people?.projectGrants.filter((g) => g.project_id === project.id);
                  return (
                    <li
                      key={project.id}
                      className="space-y-1.5 rounded-lg border border-[color:var(--border-subtle)] p-3"
                    >
                      <div className="flex items-center gap-2 text-sm">
                        <Folder className="size-3.5 flex-none text-muted-foreground" aria-hidden />
                        <span className="min-w-0 flex-1 truncate font-mono font-medium">
                          {project.name}
                        </span>
                      </div>
                      {grants && grants.length === 0 && (
                        <p className="text-xs text-muted-foreground">
                          {t("pages.teams.sheet.noProjectGrants")}
                        </p>
                      )}
                      {grants && grants.length > 0 && (
                        <ul
                          aria-label={t("pages.teams.sheet.projectPeople", { name: project.name })}
                          className="space-y-1.5"
                        >
                          {grants.map(grantRow)}
                        </ul>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          <section className="space-y-2" aria-labelledby={`team-${team.id}-more`}>
            <Overline as="h3" id={`team-${team.id}-more`}>
              {t("pages.teams.sheet.elsewhere")}
            </Overline>
            <div className="flex flex-wrap gap-x-4 gap-y-1.5 text-sm">
              <Link
                to="/gov-users"
                className="inline-flex items-center gap-1 underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t("nav.gov-users")}
                <ArrowUpRight className="size-3.5" aria-hidden />
              </Link>
              <Link
                to={`/budgets?scope_type=team&scope_id=${encodeURIComponent(team.id)}`}
                className="inline-flex items-center gap-1 underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t("pages.teams.sheet.budgetOf", { name: team.name })}
                <ArrowUpRight className="size-3.5" aria-hidden />
              </Link>
            </div>
          </section>

          <section className="mt-auto border-t border-[color:var(--border-subtle)] pt-3">
            <GatedButton
              gate="team:delete"
              control="team-delete"
              variant="outline"
              className="text-[color:var(--status-error-text)]"
              onClick={onDelete}
            >
              <Trash2 className="h-4 w-4" aria-hidden />
              {t("pages.teams.sheet.delete", { name: team.name })}
            </GatedButton>
          </section>
        </SheetBody>
      </Sheet>
    </RowCapabilityScope>
  );
}
