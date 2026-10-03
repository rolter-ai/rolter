import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { GatewayBasePrompt } from "@/components/GatewayBasePrompt";
import { CodeBlock } from "@/components/ui/code-block";
import { KEY_ENV, renderSnippet } from "@/lib/snippets";
import { useGatewayBase } from "@/lib/use-gateway-base";

/**
 * The model a first request names when the key is not limited to any.
 *
 * `fake-llm` answers from the gateway itself with no provider and no route, so
 * the request works on a fresh install, the same reason the setup checklist
 * sends it.
 */
const FIRST_REQUEST_MODEL = "fake-llm";

/**
 * What to do with a key that was just minted: the address to send it to and a
 * request that uses it (#2217).
 *
 * The reveal dialog is the step between "mint a key" and "first call", and it
 * used to end at Done. The address comes from `useGatewayBase()` and the
 * request from `renderSnippet`, so this hands out the same gateway URL every
 * other snippet does (#2218); with none saved it asks for one rather than
 * pointing an external client at the `/gw` proxy, which needs a dashboard
 * session (#2486). The key is referenced through `ROLTER_API_KEY`
 * and never written into the snippet: a request pasted into a ticket or a chat
 * must not carry a working credential.
 *
 * `models` is the allow-list the key was minted with. An empty one means every
 * route, so the request falls back to the built-in model.
 */
export function KeyNextStep({ models }: { models: string[] }) {
  const { t } = useTranslation();
  const base = useGatewayBase();
  const titleId = React.useId();
  const model = models[0] ?? FIRST_REQUEST_MODEL;
  const request = React.useMemo(
    () => (base ? renderSnippet("curl", { model, prompt: "hi" }, base) : null),
    [model, base],
  );

  return (
    <section aria-labelledby={titleId} className="flex min-w-0 flex-col gap-2">
      <h3 id={titleId} className="text-sm font-medium">
        {t("common.secret.nextStep.title")}
      </h3>
      {base && request ? (
        <>
          <p className="text-sm text-muted-foreground">
            <Trans
              i18nKey="common.secret.nextStep.body"
              values={{ env: KEY_ENV }}
              components={{ code: <code className="font-mono text-foreground" /> }}
            />
          </p>
          {/* the OpenAI SDKs take the address with `/v1` on it */}
          <CodeBlock value={`${base.url}/v1`} label={t("common.secret.nextStep.gatewayUrl")} wrap />
          <CodeBlock value={request} language="bash" label={t("common.secret.nextStep.request")} />
        </>
      ) : (
        <GatewayBasePrompt />
      )}
    </section>
  );
}
