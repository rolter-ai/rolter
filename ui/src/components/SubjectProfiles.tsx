import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Skeleton } from "@/components/ui/skeleton";
import { Plus, X } from "lucide-react";
import {
  createAccessProfileAssignment,
  deleteAccessProfileAssignment,
  fetchAccessProfiles,
  fetchOrgAccessProfileAssignments,
  type AccessProfileAssignmentRow,
  type AccessProfileRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";

/** who holds a profile: exactly one of the two, the way the assignment API takes it */
export type ProfileSubject = { user_id: string } | { team_id: string };

/** a profile a subject holds, with the assignment that grants it */
export interface HeldProfile {
  profile: AccessProfileRow;
  assignment: AccessProfileAssignmentRow;
}

/** the cache key of the org's assignment list; the bare prefix refreshes every org's */
export const ASSIGNMENTS_KEY = (orgId?: string) =>
  (orgId ? ["access-profile-assignments", orgId] : ["access-profile-assignments"]) as string[];

function holds(subject: ProfileSubject, a: AccessProfileAssignmentRow): boolean {
  return "user_id" in subject ? a.user_id === subject.user_id : a.team_id === subject.team_id;
}

/**
 * The org's access profiles with who holds each (#3010, #3025).
 *
 * Two reads however many rows ask: the profile list, and every assignment of
 * the org's profiles in one request. The rows share them through the query
 * cache, and a row filters the assignments for its own subject.
 */
export function useOrgProfiles(orgId: string | undefined) {
  const list = useQuery({
    queryKey: ["access-profiles", orgId],
    queryFn: () => fetchAccessProfiles(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const assignments = useQuery({
    queryKey: ASSIGNMENTS_KEY(orgId),
    queryFn: () => fetchOrgAccessProfileAssignments(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  // a read that has not answered holds no rows, and is never "holds nothing"
  const pending = list.isLoading || assignments.isLoading;
  const failed = list.isError || assignments.isError;
  const held = (subject: ProfileSubject): HeldProfile[] =>
    (assignments.data ?? []).flatMap((assignment) => {
      const profile = (list.data ?? []).find((p) => p.id === assignment.profile_id);
      return profile && holds(subject, assignment) ? [{ profile, assignment }] : [];
    });
  return { profiles: list.data, held, pending, failed };
}

/** what `SubjectProfiles` asks its screen to confirm */
export interface UnassignTarget extends HeldProfile {
  /** the person or team, already named for the dialog */
  name: string;
}

/**
 * The access profiles one user or team holds, each with a remove control, and
 * the control that hands out another.
 *
 * Both gate on `access_profile_assignment` at the org, the same as the buttons
 * on the Access Profiles cards, which call the same endpoints.
 */
export function SubjectProfiles({
  orgId,
  subject,
  name,
  onAssign,
  onRemove,
}: {
  orgId: string;
  subject: ProfileSubject;
  /** the person's or team's name, so each control names what it acts on */
  name: string;
  onAssign: () => void;
  onRemove: (target: UnassignTarget) => void;
}) {
  const { t } = useTranslation();
  const org = useOrgProfiles(orgId);
  const held = org.held(subject);

  return (
    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
      {org.pending ? (
        <Skeleton width={96} height={20} radius={6} />
      ) : org.failed && held.length === 0 ? (
        <span className="text-xs text-[color:var(--text-subtle)]">
          {t("subjectProfiles.unread")}
        </span>
      ) : held.length === 0 ? (
        <span className="text-xs text-[color:var(--text-subtle)]">{t("subjectProfiles.none")}</span>
      ) : (
        <ul
          aria-label={t("subjectProfiles.listLabel", { name })}
          className="flex min-w-0 flex-wrap gap-1.5"
        >
          {held.map((h) => (
            <li key={h.assignment.id} className="flex items-center gap-1">
              <Badge tone="neutral">{h.profile.name}</Badge>
              <RowIconButton
                gate="access_profile_assignment:delete"
                control="subject-profile-remove"
                title={t("subjectProfiles.remove", { name, profile: h.profile.name })}
                aria-label={t("subjectProfiles.remove", { name, profile: h.profile.name })}
                onClick={() => onRemove({ ...h, name })}
              >
                <X className="h-3.5 w-3.5" />
              </RowIconButton>
            </li>
          ))}
        </ul>
      )}
      <RowIconButton
        gate="access_profile_assignment:create"
        control="subject-profile-assign"
        title={t("subjectProfiles.assign", { name })}
        aria-label={t("subjectProfiles.assign", { name })}
        disabled={org.pending || !org.profiles}
        onClick={onAssign}
      >
        <Plus className="h-3.5 w-3.5" />
      </RowIconButton>
    </div>
  );
}

/**
 * Hand one profile to a user or team already chosen (#3010): the Access
 * Profiles sheet picks the subject for a profile, this picks the profile for a
 * subject, through the same assignment endpoint. Profiles the subject already
 * holds are not offered.
 */
export function AssignSubjectProfileSheet({
  orgId,
  subject,
  name,
  onClose,
}: {
  orgId: string;
  subject: ProfileSubject;
  name: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const org = useOrgProfiles(orgId);
  const [profileId, setProfileId] = React.useState("");

  const heldIds = new Set(org.held(subject).map((h) => h.profile.id));
  const options = (org.profiles ?? [])
    .filter((p) => !heldIds.has(p.id))
    .map((p) => ({ value: p.id, label: p.name, description: p.description ?? undefined }));
  const chosen = org.profiles?.find((p) => p.id === profileId);

  const assign = useMutation({
    mutationFn: () => createAccessProfileAssignment(profileId, subject),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["access-profiles"] });
      void queryClient.invalidateQueries({ queryKey: ASSIGNMENTS_KEY(orgId) });
      toast.push({
        tone: "success",
        title: t("subjectProfiles.assigned", { profile: chosen?.name, name }),
      });
      onClose();
    },
  });

  return (
    <EditorSheet
      name="subject-profile-assign"
      open
      onOpenChange={(open) => !open && onClose()}
      title={t("subjectProfiles.sheetTitle", { name })}
      subtitle={
        "team_id" in subject
          ? t("subjectProfiles.sheetSubtitleTeam")
          : t("subjectProfiles.sheetSubtitleUser")
      }
      dirty={profileId !== ""}
      errorMessage={assign.error ? t("subjectProfiles.failed") : undefined}
      errorDetail={assign.error ? errorText(assign.error, t) : undefined}
      saveLabel={t("pages.accessProfiles.assign.save")}
      canSave={profileId !== ""}
      saving={assign.isPending}
      onSubmit={() => assign.mutate()}
    >
      <Field label={t("subjectProfiles.field")} hint={t("subjectProfiles.fieldHint")} required>
        <Combobox
          options={options}
          value={profileId}
          onChange={(next) => {
            setProfileId(next);
            assign.reset();
          }}
          emptyText={
            org.failed ? t("subjectProfiles.unavailable") : t("subjectProfiles.nothingLeft")
          }
        />
      </Field>
    </EditorSheet>
  );
}

/** Confirm taking a profile away from a user or team, then do it. */
export function UnassignSubjectProfileDialog({
  target,
  onClose,
}: {
  target: UnassignTarget | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  // the dialog stays mounted while it closes, so it keeps the last target to
  // name in its fade-out rather than flashing empty copy
  const last = React.useRef(target);
  if (target) last.current = target;
  const shown = target ?? last.current;

  const unassign = useMutation({
    mutationFn: (item: UnassignTarget) => deleteAccessProfileAssignment(item.assignment.id),
    onSuccess: (_void, done) => {
      void queryClient.invalidateQueries({ queryKey: ["access-profiles"] });
      void queryClient.invalidateQueries({ queryKey: ASSIGNMENTS_KEY() });
      toast.push({
        tone: "success",
        title: t("pages.accessProfiles.assign.removed", { name: done.name }),
      });
      onClose();
    },
    onError: (error, failed) =>
      toast.push({
        tone: "error",
        title: t("pages.accessProfiles.assign.removeFailed", { name: failed.name }),
        detail: errorText(error, t),
      }),
  });

  return (
    <ConfirmDialog
      name="subject-profile-unassign"
      open={!!target}
      onOpenChange={(open) => {
        if (!open) {
          unassign.reset();
          onClose();
        }
      }}
      title={t("pages.accessProfiles.assign.confirmTitle", {
        name: shown?.name,
        profile: shown?.profile.name,
      })}
      description={t("pages.accessProfiles.assign.confirmBody")}
      confirmLabel={t("pages.accessProfiles.assign.confirmAction")}
      pending={unassign.isPending}
      error={unassign.error}
      onConfirm={() => target && unassign.mutate(target)}
    />
  );
}
