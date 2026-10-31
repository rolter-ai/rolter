import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Wrench } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { FormSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { CodeBlock } from "@/components/ui/code-block";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Overline } from "@/components/ui/overline";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import {
  callMcpServerTool,
  fetchMcpServerTools,
  type McpConnectionTest,
  type McpServerRow,
  type McpServerToolRow,
  type McpToolCallResult,
} from "@/lib/api";
import { describeError } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { discoveryReasonKey } from "@/lib/mcp-discovery";
import {
  argumentsSkeleton,
  connectionView,
  failureLabel,
  parseToolArguments,
  prettyPayload,
  probeLabel,
  probeTone,
  truncatedPayload,
} from "@/lib/mcp-connection";

const statusKey = (code: string | null | undefined) =>
  `pages.mcpCatalog.probe.statuses.${probeLabel(code)}`;
const explainKey = (code: string | null | undefined) =>
  `pages.mcpCatalog.probe.explain.${probeLabel(code)}`;

/** a state named in words and in a tone: the word is what carries it */
function ProbeBadge({ code }: { code: string | null | undefined }) {
  const { t } = useTranslation();
  return <Badge tone={probeTone(code)}>{t(statusKey(code))}</Badge>;
}

/**
 * Whether a server's last handshake worked, from the contact fields on its row.
 * The newest of the last contact and the last failure is the current state; a
 * failure the server has since recovered from stays visible as history.
 */
export function ConnectionHealth({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const view = connectionView(server);
  const code = view.state === "failing" ? failureLabel(view.failure ?? "unknown") : view.state;
  const when = view.at ? fmt.dateTime(view.at) : "";
  const details =
    view.state === "reachable"
      ? [
          t("pages.mcpCatalog.probe.health.contact", { when }),
          server.last_protocol_version &&
            t("pages.mcpCatalog.probe.health.protocol", { version: server.last_protocol_version }),
          server.last_server_name &&
            t("pages.mcpCatalog.probe.health.serverName", { name: server.last_server_name }),
        ].filter(Boolean)
      : view.state === "failing"
        ? [t("pages.mcpCatalog.probe.health.failedAt", { when })]
        : [t("pages.mcpCatalog.probe.health.unchecked")];
  return (
    <div
      role="group"
      aria-label={t("pages.mcpCatalog.probe.health.aria", { name: server.name })}
      className="mt-2 flex flex-col gap-1"
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <ProbeBadge code={code} />
        <span className="text-xs text-muted-foreground">{details.join(" · ")}</span>
      </div>
      {view.earlier && (
        <p className="text-xs text-[color:var(--status-warning-text)]">
          {t("pages.mcpCatalog.probe.health.lastFailure", {
            reason: t(statusKey(view.earlier.failure)),
            when: fmt.dateTime(view.earlier.at),
          })}
        </p>
      )}
    </div>
  );
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="min-w-0">
      <Overline as="dt">{label}</Overline>
      <dd className="break-words font-mono text-xs text-[color:var(--text-secondary)]">
        {children}
      </dd>
    </div>
  );
}

/** what one connection test found, in full */
export function ConnectionTestResult({
  server,
  result,
}: {
  server: McpServerRow;
  result: McpConnectionTest;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const none = t("pages.mcpCatalog.test.none");
  const yes = t("pages.mcpCatalog.probe.yes");
  const no = t("pages.mcpCatalog.probe.no");
  return (
    <div
      role="status"
      aria-label={t("pages.mcpCatalog.test.title", { name: server.name })}
      className="mt-3 rounded-[8px] border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3"
    >
      <div className="flex flex-wrap items-center gap-2">
        <ProbeBadge code={result.status} />
        <span className="text-xs text-muted-foreground">{t(explainKey(result.status))}</span>
      </div>
      <dl className="mt-2.5 grid grid-cols-2 gap-x-3 gap-y-2">
        <Fact label={t("pages.mcpCatalog.test.reachable")}>{result.reachable ? yes : no}</Fact>
        <Fact label={t("pages.mcpCatalog.test.handshake")}>
          {result.handshake_completed
            ? t("pages.mcpCatalog.test.handshakeDone")
            : t("pages.mcpCatalog.test.handshakeNot")}
        </Fact>
        <Fact label={t("pages.mcpCatalog.test.protocol")}>{result.protocol_version ?? none}</Fact>
        <Fact label={t("pages.mcpCatalog.test.serverName")}>{result.server_name ?? none}</Fact>
        <Fact label={t("pages.mcpCatalog.test.serverVersion")}>
          {result.server_version ?? none}
        </Fact>
        <Fact label={t("pages.mcpCatalog.test.latency")}>
          {t("analytics.ms", { value: fmt.number(Math.round(result.latency_ms)) })}
        </Fact>
        <Fact label={t("pages.mcpCatalog.test.checkedAt")}>{fmt.dateTime(result.checked_at)}</Fact>
      </dl>
      {result.message && (
        <div className="mt-2.5">
          <Overline>{t("pages.mcpCatalog.test.message")}</Overline>
          <p className="break-words font-mono text-xs text-[color:var(--text-secondary)]">
            {result.message}
          </p>
        </div>
      )}
    </div>
  );
}

function ToolEntry({ tool }: { tool: McpServerToolRow }) {
  const { t } = useTranslation();
  const schema = prettyPayload(tool.input_schema);
  return (
    <li className="min-w-0 rounded-[8px] border border-[color:var(--border-default)] p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="break-all font-mono text-sm font-semibold">{tool.name}</span>
        {tool.stale && <Badge tone="warning">{t("pages.mcpCatalog.tools.stale")}</Badge>}
      </div>
      <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
        {tool.description || t("pages.mcpCatalog.tools.noDescription")}
      </p>
      {schema && (
        <details className="mt-2">
          <summary className="cursor-pointer text-xs text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
            {t("pages.mcpCatalog.tools.schema")}
          </summary>
          <div className="mt-1.5">
            <CodeBlock
              value={schema}
              language="json"
              label={t("pages.mcpCatalog.tools.schemaAria", { tool: tool.name })}
              maxHeight={220}
              density="compact"
            />
          </div>
        </details>
      )}
    </li>
  );
}

function ToolList({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const tools = useQuery({
    queryKey: ["mcp-server-tools", server.id],
    queryFn: () => fetchMcpServerTools(server.id),
    retry: false,
  });
  const data = tools.data;
  if (tools.isLoading) return <FormSkeleton fields={3} />;
  if (tools.error)
    return (
      <LoadError
        error={tools.error}
        resource={t("errors.resources.mcpServerTools")}
        onRetry={() => void tools.refetch()}
        target="mcp-server-tools"
      />
    );
  if (!data) return null;
  if (!data.tools.length) {
    // an answered, empty manifest is not the absence of an answer
    const reasonKey = discoveryReasonKey(data.error);
    const [title, body] =
      data.status === "discovered"
        ? [
            t("pages.mcpCatalog.tools.declaresNoneTitle", { name: server.name }),
            t("pages.mcpCatalog.tools.declaresNoneBody"),
          ]
        : data.status === "failed"
          ? [
              t("pages.mcpCatalog.tools.failedTitle"),
              t("pages.mcpCatalog.tools.failedBody", {
                reason: t(reasonKey ?? "pages.mcpCatalog.discovery.reasons.unknown"),
              }),
            ]
          : [
              t("pages.mcpCatalog.tools.notDiscoveredTitle"),
              t("pages.mcpCatalog.tools.notDiscoveredBody"),
            ];
    return (
      <EmptyState
        uxTarget="mcp-server-tools"
        icon={<Wrench />}
        title={title}
        description={body}
        className="px-0 py-6"
      />
    );
  }
  return (
    <ul aria-label={t("pages.mcpCatalog.tools.list")} className="flex flex-col gap-2">
      {data.tools.map((tool) => (
        <ToolEntry key={tool.name} tool={tool} />
      ))}
    </ul>
  );
}

function CallResult({ result }: { result: McpToolCallResult }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const request = prettyPayload(result.request);
  const response = prettyPayload(result.response);
  const truncated = truncatedPayload(result.response);
  return (
    <div className="flex flex-col gap-3">
      <div
        role="status"
        aria-label={t("pages.mcpCatalog.try.result")}
        className="rounded-[8px] border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3"
      >
        <div className="flex flex-wrap items-center gap-2">
          <span className="sr-only">{t("pages.mcpCatalog.try.outcome")}: </span>
          <ProbeBadge code={result.outcome} />
          <span className="text-xs text-muted-foreground">
            {t("pages.mcpCatalog.try.latency")}:{" "}
            {t("analytics.ms", { value: fmt.number(Math.round(result.latency_ms)) })}
          </span>
        </div>
        <p className="mt-1.5 text-xs text-muted-foreground">{t(explainKey(result.outcome))}</p>
        {result.message && (
          <p className="mt-1.5 break-words font-mono text-xs text-[color:var(--text-secondary)]">
            {result.message}
          </p>
        )}
      </div>
      <div>
        <Overline className="mb-1">{t("pages.mcpCatalog.try.request")}</Overline>
        {request ? (
          <CodeBlock
            value={request}
            language="json"
            label={t("pages.mcpCatalog.try.request")}
            maxHeight={220}
            density="compact"
          />
        ) : (
          <p className="text-xs text-muted-foreground">{t("pages.mcpCatalog.try.noRequest")}</p>
        )}
      </div>
      <div>
        <Overline className="mb-1">{t("pages.mcpCatalog.try.response")}</Overline>
        {response ? (
          <>
            {truncated && (
              <p className="mb-1 text-xs text-[color:var(--status-warning-text)]">
                {t("pages.mcpCatalog.try.truncated", { size: fmt.number(truncated.sizeBytes) })}
              </p>
            )}
            <CodeBlock
              value={response}
              language="json"
              label={t("pages.mcpCatalog.try.response")}
              maxHeight={320}
              density="compact"
            />
          </>
        ) : (
          <p className="text-xs text-muted-foreground">{t("pages.mcpCatalog.try.noResponse")}</p>
        )}
      </div>
    </div>
  );
}

function TryIt({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  // shares the list's query, so the picker and the list can never disagree
  const tools = useQuery({
    queryKey: ["mcp-server-tools", server.id],
    queryFn: () => fetchMcpServerTools(server.id),
    retry: false,
  });
  const [tool, setTool] = React.useState("");
  const [text, setText] = React.useState("");
  const parsed = parseToolArguments(text);
  const run = useMutation({
    mutationFn: () =>
      callMcpServerTool(server.id, {
        tool,
        arguments: parsed.ok ? parsed.value : undefined,
      }),
    // a call that failed to connect is also news for the card's status
    onSettled: () => void client.invalidateQueries({ queryKey: ["mcp-servers"] }),
  });
  const list = tools.data?.tools ?? [];
  const error = run.error ? describeError(run.error, t) : null;
  const problem = parsed.ok
    ? undefined
    : parsed.reason === "invalid_json"
      ? t("pages.mcpCatalog.try.invalidJson", { detail: parsed.detail })
      : t("pages.mcpCatalog.try.notObject");
  return (
    <section aria-label={t("pages.mcpCatalog.try.title")} className="flex flex-col gap-3">
      <h3 className="text-sm font-semibold">{t("pages.mcpCatalog.try.title")}</h3>
      <p className="text-xs leading-relaxed text-muted-foreground">
        {t("pages.mcpCatalog.try.warning")}
      </p>
      <Field label={t("pages.mcpCatalog.try.tool")}>
        <Combobox
          value={tool}
          onChange={(picked) => {
            setTool(picked);
            run.reset();
            setText(argumentsSkeleton(list.find((entry) => entry.name === picked)?.input_schema));
          }}
          placeholder={t("pages.mcpCatalog.try.toolPlaceholder")}
          emptyText={t("pages.mcpCatalog.try.toolEmpty")}
          options={list.map((entry) => ({ value: entry.name, label: entry.name }))}
        />
      </Field>
      <Field
        label={t("pages.mcpCatalog.try.arguments")}
        hint={t("pages.mcpCatalog.try.argumentsHint")}
        error={problem}
      >
        <Textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          rows={6}
          spellCheck={false}
          className="font-mono text-xs"
        />
      </Field>
      <div>
        <GatedButton
          gate="mcp_server:update"
          control="mcp-server-try"
          disabled={!tool || !parsed.ok || run.isPending}
          aria-label={t("pages.mcpCatalog.try.runAria", { name: server.name })}
          onClick={() => run.mutate()}
        >
          {run.isPending && (
            <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
          )}
          {run.isPending ? t("pages.mcpCatalog.try.running") : t("pages.mcpCatalog.try.run")}
        </GatedButton>
      </div>
      {/* a 400 (a tool outside the manifest) or a refused request is the call
          not happening at all, unlike an outcome, which is the call's answer */}
      {error && (
        <div role="alert" className="text-xs">
          <p className="text-[color:var(--status-danger-text)]">
            {t("pages.mcpCatalog.try.failed")}: {error.message}
          </p>
          {error.detail && (
            <p className="mt-1 break-words font-mono text-[color:var(--text-subtle)]">
              {error.detail}
            </p>
          )}
        </div>
      )}
      {run.data && <CallResult result={run.data} />}
    </section>
  );
}

/**
 * One server's tools as the last discovery found them, with a way to run one
 * for real and read the raw JSON-RPC exchange.
 */
export function ToolsSheet({ server, onClose }: { server: McpServerRow; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <Sheet open onOpenChange={(open) => !open && onClose()}>
      <SheetHeader
        title={t("pages.mcpCatalog.tools.title", { name: server.name })}
        subtitle={server.url}
        onClose={onClose}
      />
      <SheetBody>
        <ToolList server={server} />
        <TryIt server={server} />
      </SheetBody>
    </Sheet>
  );
}
