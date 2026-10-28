import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { errorText } from "@/lib/error-copy";
import { startMcpOAuth, startMcpStepUp, type McpServerRow } from "@/lib/api";
import { useToast } from "@/lib/toast";

// starts consent and hands the browser the url the control plane minted. the
// dashboard never navigates itself there: the consent screen belongs to a third
// party, and the operator should come back to a page that kept its place. the
// MCP Catalog's Connect and an expired session's Reconnect on Auth Sessions
// are the same request, so they share this
//
// a step-up (#3296) is the same request carrying the scopes to add
export function useMcpConnect(server: Pick<McpServerRow, "id" | "name">, stepUpScopes?: string[]) {
  const { t } = useTranslation();
  const toast = useToast();
  return useMutation({
    mutationFn: () =>
      stepUpScopes?.length ? startMcpStepUp(server.id, stepUpScopes) : startMcpOAuth(server.id),
    onSuccess: (started) => {
      const opened = window.open(started.authorization_url, "_blank", "noopener,noreferrer");
      // a blocked pop-up is silent otherwise: the request succeeded, a login
      // state row exists upstream, and nothing at all appeared on screen
      if (!opened)
        return void toast.push({
          tone: "error",
          title: t("pages.mcpCatalog.connect.blocked"),
          detail: t("pages.mcpCatalog.connect.blockedDetail"),
        });
      toast.push({
        tone: "info",
        title: t("pages.mcpCatalog.connect.started", { name: server.name }),
        detail: t("pages.mcpCatalog.connect.startedDetail"),
      });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpCatalog.connect.failed", { name: server.name }),
        detail: errorText(error, t),
      }),
  });
}
