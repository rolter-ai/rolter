import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Textarea } from "@/components/ui/textarea";
import {
  ApiError,
  MAX_BIO_LEN,
  MAX_DISPLAY_NAME_LEN,
  fetchMe,
  isOpenModeNoSession,
  updateMyProfile,
  type MeResponse,
  type ProfileUpdate,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { describeError } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";

export const ME_KEY = "me";

type Problem = "blank" | "tooLong" | null;

/** what the server would refuse, said before the request is made */
function problemWith(value: string, max: number): Problem {
  if (value === "") return null;
  const trimmed = value.trim();
  if (trimmed === "") return "blank";
  return [...trimmed].length > max ? "tooLong" : null;
}

/**
 * The account's own display name and bio (#2434).
 *
 * Self-service like the second factor beside it: `/me/profile` needs no role.
 * Only the fields that changed are sent, an emptied field clears itself, and a
 * name a SCIM directory owns is shown read-only — the bio stays editable.
 */
export function ProfileCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const auth = useOptionalAuth();

  const me = useQuery({ queryKey: [ME_KEY], queryFn: fetchMe, retry: false });

  const storedName = me.data?.user.display_name ?? "";
  const storedBio = me.data?.user.bio ?? "";
  const managed = me.data?.display_name_managed === true;

  const [name, setName] = React.useState("");
  const [bio, setBio] = React.useState("");
  // what the server holds replaces the draft whenever it changes: first load,
  // and the trimmed values a save answers with
  React.useEffect(() => {
    setName(storedName);
    setBio(storedBio);
  }, [storedName, storedBio]);

  const nameProblem = managed ? null : problemWith(name, MAX_DISPLAY_NAME_LEN);
  const bioProblem = problemWith(bio, MAX_BIO_LEN);
  const nameChanged = !managed && name.trim() !== storedName;
  const bioChanged = bio.trim() !== storedBio;
  const canSave = (nameChanged || bioChanged) && !nameProblem && !bioProblem;

  const save = useMutation({
    mutationFn: () => {
      const body: ProfileUpdate = {};
      // null clears; an omitted field is left as it is
      if (nameChanged) body.display_name = name.trim() || null;
      if (bioChanged) body.bio = bio.trim() || null;
      return updateMyProfile(body);
    },
    onSuccess: (profile) => {
      queryClient.setQueryData<MeResponse>([ME_KEY], (current) =>
        current
          ? {
              ...current,
              user: { ...current.user, display_name: profile.display_name, bio: profile.bio },
              display_name_managed: profile.display_name_managed,
            }
          : current,
      );
      auth?.applyProfile(profile);
      toast.push({ tone: "success", title: t("account.profile.saved") });
    },
  });

  // open mode has no accounts, so there is no profile to edit and the keys
  // panel already says why the screen is inert; an older control plane that
  // does not mount /auth/me answers 404 for the same reason (#942)
  if (isOpenModeNoSession(me.error)) return null;
  if (me.error instanceof ApiError && me.error.status === 404) return null;

  const problemText = (field: "name" | "bio", problem: Problem) =>
    problem === null
      ? undefined
      : t(`account.profile.problem.${field}.${problem}`, {
          max: field === "name" ? MAX_DISPLAY_NAME_LEN : MAX_BIO_LEN,
        });

  return (
    <SettingsPanel title={t("account.profile.title")} description={t("account.profile.subtitle")}>
      {me.isLoading && <PanelSkeleton panels={1} height={160} />}
      {me.error && (
        <LoadError
          error={me.error}
          resource={t("errors.resources.yourProfile")}
          onRetry={() => void me.refetch()}
        />
      )}
      {me.data && (
        <form
          className="flex w-full flex-col gap-3.5"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSave && !save.isPending) save.mutate();
          }}
        >
          <Field
            label={t("account.profile.name")}
            error={problemText("name", nameProblem)}
            hint={managed ? t("account.profile.managed") : t("account.profile.nameHint")}
          >
            <Input
              value={name}
              readOnly={managed}
              maxLength={MAX_DISPLAY_NAME_LEN * 2}
              autoComplete="nickname"
              placeholder={t("account.profile.namePlaceholder")}
              // a line, not a paragraph: pasted newlines are folded to spaces
              onChange={(e) => setName(e.target.value.replace(/\s*[\r\n]+\s*/g, " "))}
            />
          </Field>
          <Field
            label={t("account.profile.bio")}
            error={problemText("bio", bioProblem)}
            hint={t("account.profile.bioHint", {
              count: [...bio.trim()].length,
              max: MAX_BIO_LEN,
            })}
          >
            <Textarea
              value={bio}
              rows={3}
              placeholder={t("account.profile.bioPlaceholder")}
              onChange={(e) => setBio(e.target.value)}
            />
          </Field>
          {save.isError && (
            <div role="alert">
              <FieldError
                id="profile-save-error"
                error={[
                  t("account.profile.saveFailed"),
                  describeError(save.error, t).message,
                  describeError(save.error, t).detail,
                ]
                  .filter(Boolean)
                  .join(" ")}
              />
            </div>
          )}
          <div>
            <Button type="submit" size="sm" disabled={!canSave || save.isPending}>
              {t("account.profile.save")}
            </Button>
          </div>
        </form>
      )}
    </SettingsPanel>
  );
}
