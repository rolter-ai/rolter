import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";

/**
 * "Not saved", beside a control whose new value waits for Save (#2214).
 *
 * A switch on a settings screen moves the moment it is pressed, and nothing
 * else says the gateways still run the old value. The worst case is a kill
 * switch flipped in the middle of an incident and never saved. `SettingsPanel`
 * and `SwitchRow` take `unsaved` and draw this, so a switch cannot look
 * immediate where it is not.
 */
export function UnsavedBadge() {
  const { t } = useTranslation();
  return <Badge tone="warning">{t("common.settings.notSaved")}</Badge>;
}
