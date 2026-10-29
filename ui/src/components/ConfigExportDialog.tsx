import { useMutation, useQuery } from "@tanstack/react-query";
import { Download, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { SwitchRow } from "@/components/ui/switch-row";
import { ApiError, CONFIG_EXPORT_NEEDS_ORG, exportConfigToml, fetchOrgs } from "@/lib/api";
import { useFormTelemetry } from "@/lib/ux-react";

// the picker's "every organization" entry. a slug is lowercase letters, digits
// and dashes, so no organization can collide with it, and the combobox reads an
// empty value as "nothing chosen", which is what a required pick starts as
const ALL = "*";

/**
 * Choose what a `rolter.toml` export carries: the whole deployment or one
 * organization's rows (#3440), and whether caps on a person, which name an
 * account by email, stay out of the file (#3396).
 *
 * The whole deployment is the default. A deployment where two organizations
 * share a route, provider or slug cannot be one file, and the control plane
 * refuses it with `config_export_needs_org` (#3480): the dialog then says to
 * pick an organization, drops "All organizations" from the list and asks for a
 * pick, until the screen is left. It reads the organizations only while open.
 */
export function ConfigExportDialog({
  open,
  onOpenChange,
  onExported,
  onFailed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the document the control plane rendered, to be saved by the caller */
  onExported: (toml: string) => void;
  /** a failure other than the refusal this dialog answers itself */
  onFailed: (error: unknown) => void;
}) {
  const { t } = useTranslation();
  const [org, setOrg] = React.useState(ALL);
  const [withoutPersonCaps, setWithoutPersonCaps] = React.useState(false);
  // the refusal outlives the dialog: once the control plane has said the whole
  // deployment is not one file, offering it again on the next open is a trap
  const [needsOrg, setNeedsOrg] = React.useState(false);

  const orgs = useQuery({
    queryKey: ["scope", "orgs"],
    queryFn: fetchOrgs,
    enabled: open,
  });

  const run = useMutation({
    mutationFn: exportConfigToml,
    onSuccess: (toml) => {
      onExported(toml);
      onOpenChange(false);
    },
    onError: (error) => {
      if (error instanceof ApiError && error.code === CONFIG_EXPORT_NEEDS_ORG) {
        setNeedsOrg(true);
        setOrg("");
        return;
      }
      onFailed(error);
    },
  });
  const reset = run.reset;
  React.useEffect(() => {
    if (open) reset();
  }, [open, reset]);

  const ux = useFormTelemetry("config-export", open, { dirty: false });

  const options: ComboboxOption[] = [
    ...(needsOrg ? [] : [{ value: ALL, label: t("pages.config.export.allOrgs") }]),
    ...(orgs.data ?? []).map((row) => ({
      value: row.slug,
      label: row.name,
      description: row.slug,
    })),
  ];
  const noOrgs = orgs.isSuccess && orgs.data.length === 0;
  // said under the picker it is about, and gone once an organization is picked
  const refusal = needsOrg && !noOrgs && org === "" ? t("pages.config.export.needsOrg") : undefined;

  const submit = () => {
    ux.submitted();
    run.mutate(
      {
        org: org === ALL || org === "" ? undefined : org,
        withoutPersonCaps,
      },
      {
        onSuccess: () => ux.saved(),
        onError: () => ux.failed(),
      },
    );
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      onSubmit={submit}
      canSubmit={!orgs.isLoading && !refusal}
      busy={run.isPending}
    >
      <DialogHeader>
        <DialogTitle>{t("pages.config.export.title")}</DialogTitle>
        <DialogDescription>{t("pages.config.export.body")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">
        {orgs.isLoading ? (
          <Field label={t("pages.config.export.org")}>
            <ControlSkeleton width="100%" />
          </Field>
        ) : orgs.isError ? (
          <LoadError
            error={orgs.error}
            resource={t("errors.resources.organizations")}
            onRetry={() => orgs.refetch()}
            target="config-export-orgs"
          />
        ) : noOrgs ? (
          <p
            role="note"
            className="rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground"
          >
            {t("pages.config.export.noOrgs")}
          </p>
        ) : (
          <Field
            label={t("pages.config.export.org")}
            hint={t("pages.config.export.orgHint")}
            error={refusal}
            required={needsOrg}
          >
            <Combobox
              options={options}
              value={org}
              onChange={setOrg}
              placeholder={t("pages.config.export.orgPlaceholder")}
            />
          </Field>
        )}
        <SwitchRow
          title={t("pages.config.export.withoutPersonCaps")}
          hint={t("pages.config.export.withoutPersonCapsHint")}
          checked={withoutPersonCaps}
          onChange={setWithoutPersonCaps}
        />
      </DialogBody>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          {t("common.cancel")}
        </Button>
        <GatedButton
          type="submit"
          gate="config_export:read"
          control="config-export-run"
          disabled={run.isPending}
        >
          {run.isPending ? (
            <Loader2 aria-hidden className="mr-2 h-4 w-4 motion-safe:animate-spin" />
          ) : (
            <Download aria-hidden className="mr-2 h-4 w-4" />
          )}
          {run.isPending ? t("pages.config.export.pending") : t("pages.config.export.run")}
        </GatedButton>
      </DialogFooter>
    </Dialog>
  );
}
