import type { TFunction } from "i18next";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { SaveBar } from "@/components/ui/save-bar";
import type { Pickup } from "@/lib/gateway-pickup";
import { useFormat } from "@/lib/i18n/format";
import { useSettingsPickup } from "@/lib/use-settings-pickup";

/** What the footer reads off `useSettingsForm`. */
interface SettingsFormState {
  dirty: boolean;
  changed: readonly unknown[];
  discard: () => void;
  savedAt: number | null;
  leavePrompt: React.ReactNode;
  fields: { invalid: readonly unknown[] };
}

export interface SettingsSaveBarProps {
  form: SettingsFormState;
  /** a save is in flight */
  pending: boolean;
  /** the DTO's `updated_at`: when the stored settings were last written, by anyone */
  updatedAt?: string | null;
  /**
   * Whether a gateway reads this setting. A save is then followed by the
   * "applied to N of M gateways" line, from the config versions the Cluster
   * screen reads. Off for a setting only the control plane uses.
   */
  gateways?: boolean;
  /** pinned to the start of the bar */
  leading?: React.ReactNode;
  /** secondary buttons beside Discard */
  children?: React.ReactNode;
}

/**
 * The footer every deployment-settings screen shares (#2214).
 *
 * Save waits for an edit and stays pressable while a field is wrong (it is the
 * form's submit, so Enter in a field presses it too); Discard puts every field
 * back; the line beside them says what state the screen is in: how many changes
 * are not saved, or that they were, when, and how many live gateways already
 * run them. A screen that is left with a draft asks first (`form.leavePrompt`).
 */
export function SettingsSaveBar({
  form,
  pending,
  updatedAt,
  gateways = true,
  leading,
  children,
}: SettingsSaveBarProps) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const pickup = useSettingsPickup(gateways ? form.savedAt : null);

  let status: string | undefined;
  if (form.dirty) {
    status = t("common.settings.unsaved", { count: form.changed.length });
  } else if (form.savedAt !== null) {
    const saved = t("common.settings.savedAt", { time: fmt.time(form.savedAt) });
    status = pickup ? `${saved} ${pickupLine(t, pickup)}` : saved;
  } else if (updatedAt) {
    status = t("common.settings.lastSaved", { when: fmt.dateTime(updatedAt) });
  }

  return (
    <>
      <SaveBar
        invalid={form.fields.invalid.length}
        pending={pending}
        disabled={!form.dirty}
        status={status}
        leading={leading}
      >
        {children}
        <Button variant="outline" disabled={!form.dirty || pending} onClick={form.discard}>
          {t("common.settings.discard")}
        </Button>
      </SaveBar>
      {form.leavePrompt}
    </>
  );
}

// what the gateways say about the save just made, in words true of each
// answer: only a gateway the control plane has heard from can be counted
function pickupLine(t: TFunction, pickup: Pickup): string {
  switch (pickup.state) {
    case "checking":
      return t("common.settings.pickup.checking");
    case "unavailable":
      return t("common.settings.pickup.unavailable");
    case "none":
      return t("common.settings.pickup.none");
    case "converged":
      return t("common.settings.pickup.converged", { count: pickup.total });
    case "lagging":
      return t("common.settings.pickup.lagging", {
        count: pickup.total,
        converged: pickup.converged,
      });
  }
}
