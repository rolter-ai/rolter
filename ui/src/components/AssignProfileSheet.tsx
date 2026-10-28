import { ASSIGNMENTS_KEY } from "@/components/SubjectProfiles";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { EditorSheet } from "@/components/EditorSheet";
import { useOrgScope } from "@/components/OrgScopePicker";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import {
  createAccessProfileAssignment,
  fetchUsers,
  type AccessProfileAssignmentRow,
  type AccessProfileRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useOptionalAuth } from "@/lib/auth";
import { useToast } from "@/lib/toast";

type Kind = "user" | "team";

/**
 * Hand an access profile to one user or one team (#2073).
 *
 * A team assignment reaches every member of the team, which is the point of a
 * profile over a direct grant. The picker offers only subjects the profile does
 * not already reach: the control plane would take a duplicate, and a list that
 * invites it is a list that hides the reach it already has.
 */
export function AssignProfileSheet({
  profile,
  assigned,
  onClose,
}: {
  profile: AccessProfileRow;
  assigned: AccessProfileAssignmentRow[];
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [kind, setKind] = React.useState<Kind>("user");
  const [subject, setSubject] = React.useState("");

  // a profile grants inside its own org, so both lists are that org's. a
  // superadmin also gets the accounts that hold no membership anywhere, such as
  // the first administrator the seeder creates, which are otherwise nowhere to
  // pick (#3010); the control plane ignores the flag for anyone else. the flag
  // is in the key so the plain list other screens cache is not stood in for
  const callerIsSuperadmin = !!useOptionalAuth()?.user?.is_superadmin;
  const users = useQuery({
    queryKey: ["users", profile.org_id, { includeUnassigned: callerIsSuperadmin }],
    queryFn: () => fetchUsers(profile.org_id, { includeUnassigned: callerIsSuperadmin }),
    retry: false,
  });
  const orgScope = useOrgScope(profile.org_id);

  const taken = new Set(assigned.map((a) => a.user_id ?? a.team_id));
  const options =
    kind === "user"
      ? (users.data ?? [])
          .filter((u) => !taken.has(u.id))
          .map((u) => ({
            value: u.id,
            label: u.display_name || u.email,
            description: u.display_name ? u.email : undefined,
          }))
      : orgScope.teams
          .filter((team) => !taken.has(team.id))
          .map((team) => ({
            value: team.id,
            label: team.name,
          }));

  const assign = useMutation({
    mutationFn: () =>
      createAccessProfileAssignment(
        profile.id,
        kind === "user" ? { user_id: subject } : { team_id: subject },
      ),
    onSuccess: () => {
      // the Access Profiles cards are drawn from the list read (#3301)
      void queryClient.invalidateQueries({ queryKey: ["access-profiles"] });
      void queryClient.invalidateQueries({ queryKey: ASSIGNMENTS_KEY() });
      toast.push({
        tone: "success",
        title: t("pages.accessProfiles.assign.assigned", { name: profile.name }),
      });
      onClose();
    },
  });

  const unavailable =
    kind === "user" ? users.isError : !!orgScope.error && orgScope.teams.length === 0;

  return (
    <EditorSheet
      name="access-profile-assign"
      open
      onOpenChange={(open) => !open && onClose()}
      title={t("pages.accessProfiles.assign.title", { name: profile.name })}
      subtitle={t("pages.accessProfiles.assign.subtitle")}
      dirty={subject !== ""}
      errorMessage={assign.error ? t("pages.accessProfiles.assign.failed") : undefined}
      errorDetail={assign.error ? errorText(assign.error, t) : undefined}
      saveLabel={t("pages.accessProfiles.assign.save")}
      canSave={subject !== ""}
      saving={assign.isPending}
      onSubmit={() => assign.mutate()}
    >
      <div className="space-y-3.5">
        <Segmented<Kind>
          ariaLabel={t("pages.accessProfiles.assign.kindLabel")}
          value={kind}
          options={[
            { value: "user", label: t("pages.accessProfiles.assign.kindUser") },
            { value: "team", label: t("pages.accessProfiles.assign.kindTeam") },
          ]}
          onChange={(next) => {
            setKind(next);
            setSubject("");
            assign.reset();
          }}
        />
        <Field
          label={
            kind === "user"
              ? t("pages.accessProfiles.assign.fieldUser")
              : t("pages.accessProfiles.assign.fieldTeam")
          }
          hint={
            kind === "user"
              ? t("pages.accessProfiles.assign.userHint")
              : t("pages.accessProfiles.assign.teamHint")
          }
          required
        >
          <Combobox
            options={options}
            value={subject}
            onChange={(next) => {
              setSubject(next);
              assign.reset();
            }}
            emptyText={
              unavailable
                ? t("pages.accessProfiles.assign.unavailable")
                : t("pages.accessProfiles.assign.nobodyLeft")
            }
          />
        </Field>
      </div>
    </EditorSheet>
  );
}
