import { useTranslation } from "react-i18next";

/**
 * What a snippet screen shows when there is no gateway address to put in one
 * (#2486).
 *
 * A snippet is for a client outside the browser, and the dashboard's `/gw`
 * proxy is no address for it: the proxy requires a dashboard session. So with
 * no public base URL saved on Client Settings — or a caller who may not read
 * it — the screen says what is missing rather than printing a `/gw` URL that
 * answers 401 from anywhere else.
 */
/**
 * Says that the address a snippet uses is the gateway on this machine, the
 * loopback one the control plane reaches it at, rather than a saved public
 * base URL (#3650). It works for a client beside the gateway and for no one
 * else, and the note says where the address for everyone else is set.
 */
export function LocalGatewayNote() {
  const { t } = useTranslation();
  return (
    <p role="note" className="text-xs text-muted-foreground">
      {t("common.secret.nextStep.localNote")}
    </p>
  );
}

export function GatewayBasePrompt() {
  const { t } = useTranslation();
  return (
    <p role="note" className="text-sm text-muted-foreground">
      {t("common.gatewayBasePrompt")}
    </p>
  );
}
