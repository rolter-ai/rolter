import { useMutation, useQuery } from "@tanstack/react-query";
import {
  GitCompare,
  ImageIcon,
  Mic,
  Paperclip,
  Pilcrow,
  Play,
  Plus,
  Send,
  Trash2,
  Upload,
  Loader2,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { CopyAsCodeButton } from "@/components/CodeSnippetDialog";
import { DocsLink } from "@/components/DocsLink";
import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Input } from "@/components/ui/input";
import { ScatterPlot, type ScatterPoint } from "@/components/ui/scatter-plot";
import { StatusRow } from "@/components/ui/status-row";
import { Switch } from "@/components/ui/switch";
import { Tabs } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { fetchModels, mintPlaygroundKey } from "@/lib/api";
import {
  chatCompletion,
  embed,
  fetchGatewayModels,
  generateImages,
  getPlaygroundKeyState,
  realtimeUrl,
  setPlaygroundKey,
  subscribePlaygroundKey,
  synthesizeSpeech,
  transcribe,
  type ChatMessage,
  type GeneratedImage,
  type PlaygroundKeyState,
} from "@/lib/gateway";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useScreenReady } from "@/lib/ux-react";

// the built-in fake-llm always works with no upstream/secrets, so it's a safe
// default for every modality in local dev.
const FAKE = "fake-llm";

/** One selectable address, with whatever the gateway said owns it. */
export interface ModelOption {
  id: string;
  /** `owned_by` from `/v1/models`; absent when the list came from routes */
  ownedBy?: string;
}

/**
 * Where the picker's contents came from. The distinction matters because the
 * two sources do not list the same things, and quietly swapping one for the
 * other is what #946 is about.
 */
export type ModelSource = "gateway" | "no-key" | "unreachable";

/**
 * Every id the gateway will actually accept.
 *
 * The control plane's `/api/v1/models` lists configured *routes*. The gateway
 * also serves `provider-slug/model` pins and `group-slug/model` provider-group
 * addresses, so listing routes alone left a provider group impossible to select
 * here — on the one screen whose job is to send it a request (#938).
 *
 * The gateway call needs a virtual key. Without one — or with one the gateway
 * rejects — the route list is still shown, because an empty dropdown helps
 * nobody, but the caller is told which source it got (#946). Silently
 * substituting a strictly smaller list was the bug: a provider group simply
 * vanished, with nothing on screen to say why.
 */
function useModelCatalog(): { options: ModelOption[]; source: ModelSource; ready: boolean } {
  // the key is read through the store rather than once at render, so the list
  // re-fetches the moment the screen mints one (#944)
  const key = usePlaygroundKeyState().key;
  const routes = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  const gateway = useQuery({
    queryKey: ["gateway-models", key],
    queryFn: ({ signal }) => fetchGatewayModels(signal),
    enabled: !!key,
    retry: false,
  });

  const source: ModelSource = gateway.data ? "gateway" : key ? "unreachable" : "no-key";

  const options: ModelOption[] = gateway.data
    ? gateway.data.map((m) => ({ id: m.id, ownedBy: m.owned_by }))
    : (routes.data ?? []).map((m) => ({ id: m.model }));

  // the built-in always works, so it stays selectable whatever the source
  const withFake = options.some((o) => o.id === FAKE)
    ? options
    : [{ id: FAKE, ownedBy: "rolter" }, ...options];
  // the catalog is what the screen waits on before anything can be sent, so
  // it is the query `time_to_interactive` should be measured against. the
  // gateway probe only counts when there is a key to make it with — an
  // `enabled: false` query stays pending forever and would suppress the event
  const ready = !routes.isPending && (!key || !gateway.isPending);
  return { options: withFake, source, ready };
}

/** Routes have bare ids; pins and groups are addressed `owner/model`. */
function groupLabel(option: ModelOption, routesLabel: string): string {
  if (!option.id.includes("/")) return routesLabel;
  return option.ownedBy ?? option.id.split("/")[0];
}

function ModelSelect({
  models,
  value,
  onChange,
  className,
}: {
  models: ModelOption[];
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  const { t } = useTranslation();
  const routesLabel = t("pages.playground.groupRoutes");
  // routes, provider pins and groups look identical as bare strings, so they
  // are grouped by owner — `owned_by` already carries what is needed (#946).
  // the combobox lays groups out in first-seen order, so the options are
  // sorted into their buckets first (#968)
  const groups = new Map<string, ComboboxOption[]>();
  for (const option of models) {
    const label = groupLabel(option, routesLabel);
    const row: ComboboxOption = { value: option.id, label: option.id, group: label };
    const bucket = groups.get(label);
    if (bucket) bucket.push(row);
    else groups.set(label, [row]);
  }

  return (
    <Combobox
      options={[...groups.values()].flat()}
      value={value}
      onChange={onChange}
      aria-label={t("pages.playground.modelAria")}
      size="sm"
      className={className}
    />
  );
}

/**
 * Says which list the picker is showing when it is not the gateway's.
 *
 * The fallback itself is fine — an empty dropdown would be worse — but it
 * lists routes only, so provider pins and provider groups are missing from
 * it. Without this notice they just vanish, which reads as the group not
 * existing rather than as a list rolter could not fetch (#946).
 */
function ModelSourceNotice({ source }: { source: ModelSource }) {
  const { t } = useTranslation();
  if (source === "gateway") return null;
  return (
    <p
      role="status"
      className="rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground"
    >
      {source === "no-key"
        ? t("pages.playground.modelsNeedKey")
        : t("pages.playground.modelsUnreachable")}
    </p>
  );
}

/* ---------------- session key bar ---------------- */

/**
 * The key the Playground is sending, as a store rather than component state.
 *
 * It lives in `lib/gateway.ts` because every call on this screen authenticates
 * with it, and in memory because #944 is about a gateway credential that used
 * to be written to `localStorage` and stay there long after the sitting that
 * needed it.
 */
function usePlaygroundKeyState(): PlaygroundKeyState {
  return React.useSyncExternalStore(
    subscribePlaygroundKey,
    getPlaygroundKeyState,
    getPlaygroundKeyState,
  );
}

/**
 * The current time, re-read every `intervalMs`.
 *
 * A minted key is good for half an hour, so "expires in 29 min" has to count
 * down on its own — a countdown that only moves when something else re-renders
 * is how a key reads as live several minutes after it stopped working.
 */
function useNow(intervalMs = 15_000): number {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/**
 * Mints the Playground's key and says what state it is in.
 *
 * Opening the screen as a signed-in operator mints a key scoped by the control
 * plane to the routes of the project in scope, so the five-second smoke test
 * this screen exists for does not start with a trip to the Keys screen and a
 * paste. The secret is never rendered: the key is held in memory and shown only
 * as its state, because there is nothing an operator does with the string that
 * the screen is not already doing for them.
 *
 * The paste field stays, for testing one specific key on purpose — the case
 * automatic minting cannot serve.
 */
function SessionKeyBar() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  const state = usePlaygroundKeyState();
  const now = useNow();
  const projectId = scope.projectId;

  const mint = useMutation({
    mutationFn: () => mintPlaygroundKey(projectId as string),
    onSuccess: (minted) =>
      setPlaygroundKey(minted.key, { expiresAt: minted.expires_at ?? null, minted: true }),
  });

  // one automatic attempt per project, not one per render: a refusal — a
  // project with no routes answers 400 — must not turn into a mint loop, and
  // the operator renews by hand from here on
  const { mutate } = mint;
  const asked = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!projectId || state.key || asked.current === projectId) return;
    asked.current = projectId;
    mutate();
  }, [projectId, state.key, mutate]);

  const expired = state.expiresAt !== null && new Date(state.expiresAt).getTime() <= now;

  return (
    // the failure sits outside the band rather than inside it: `LoadError`
    // paints the control plane's own message in `--text-subtle`, which clears
    // AA on the page surface and not on the lighter `--surface-subtle` (#1725)
    <>
      <div className="flex flex-col gap-2 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-medium text-muted-foreground">
            {t("playground.key.title")}
          </span>
          {mint.isPending ? (
            <ControlSkeleton width={132} />
          ) : (
            <KeyStatus state={state} expired={expired} />
          )}
          {!mint.isPending && state.minted && !expired && state.expiresAt && (
            <span className="text-xs text-[color:var(--text-subtle)]">
              {t("playground.key.expires", { when: fmt.relative(state.expiresAt, now) })}
            </span>
          )}
          <Button
            size="sm"
            variant="outline"
            className="ml-auto"
            disabled={!projectId || mint.isPending}
            onClick={() => mint.mutate()}
          >
            {mint.isPending && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
            {t("playground.key.renew")}
          </Button>
        </div>
        {/* the project chain has to resolve before there is anything to mint
          against, and an operator who belongs to no project needs to hear that
          rather than watch a button do nothing */}
        {!projectId && !scope.isLoading && (
          <p className="text-xs text-[color:var(--text-subtle)]">{t("playground.key.noProject")}</p>
        )}
        {!mint.isPending && !mint.error && (
          <p className="text-xs leading-snug text-[color:var(--text-subtle)]">
            {expired ? t("playground.key.expiredHint") : t("playground.key.mintedHint")}
          </p>
        )}
        <ManualKeyField pasted={state.key !== "" && !state.minted} />
      </div>
      {mint.error != null && (
        <LoadError
          error={mint.error}
          resource={t("errors.resources.playgroundKey")}
          onRetry={() => mint.mutate()}
        />
      )}
    </>
  );
}

/** What the screen is currently sending, in one badge. */
function KeyStatus({ state, expired }: { state: PlaygroundKeyState; expired: boolean }) {
  const { t } = useTranslation();
  if (!state.key) return <Badge tone="neutral">{t("playground.key.none")}</Badge>;
  if (!state.minted)
    return (
      <Badge tone="info" dot>
        {t("playground.key.pasted")}
      </Badge>
    );
  return expired ? (
    <Badge tone="warning" dot>
      {t("playground.key.expired")}
    </Badge>
  ) : (
    <Badge tone="success" dot>
      {t("playground.key.active")}
    </Badge>
  );
}

/**
 * The manual paste field, collapsed by default.
 *
 * Kept because testing one particular key — a customer's, a key that is about
 * to expire — is a real thing to do here, and automatic minting cannot do it.
 * Collapsed because it is now the exception: the screen arrives with a key.
 */
function ManualKeyField({ pasted }: { pasted: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(pasted);
  const [key, setKey] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const save = () => {
    setPlaygroundKey(key.trim());
    setSaved(true);
    setTimeout(() => setSaved(false), 1400);
  };
  return (
    <div className="flex flex-col gap-2">
      <Button
        size="sm"
        variant="ghost"
        className="self-start px-1 text-xs text-muted-foreground"
        aria-expanded={open}
        aria-controls="playground-manual-key"
        onClick={() => setOpen((was) => !was)}
      >
        {t("playground.key.manual")}
      </Button>
      {open && (
        <div id="playground-manual-key" className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <span className="text-xs font-medium text-muted-foreground">
              {t("playground.key.label")}
            </span>
            <Input
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder={t("playground.key.placeholder")}
              className="h-8 flex-1 font-mono text-xs"
              type="password"
              spellCheck={false}
              aria-label={t("playground.key.label")}
            />
            <Button size="sm" variant="outline" onClick={save}>
              {saved ? t("playground.key.saved") : t("playground.key.save")}
            </Button>
          </div>
          {/* three different credentials in this product answer to "api key";
              say which one this field wants (#943) */}
          <p className="text-[0.6875rem] leading-snug text-[color:var(--text-subtle)]">
            {t("playground.key.hint")}{" "}
            {/* suppressed entirely when no documentation host is configured, so
                the hint above never trails a dead link (#1164) */}
            <DocsLink page="whichKey" label={t("docs.link.whichKey")} />
          </p>
        </div>
      )}
    </div>
  );
}

function ErrorNote({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p className="rounded-md border border-[color:var(--status-danger)]/40 bg-destructive/10 px-3 py-2 text-xs text-[color:var(--status-danger-text)]">
      {error}
    </p>
  );
}

/* ---------------- chat column ---------------- */
interface Msg {
  role: "user" | "assistant";
  text: string;
  pending?: boolean;
}

function ChatColumn({
  models,
  model,
  onModel,
  onRemove,
  removable,
  multimodal,
}: {
  models: ModelOption[];
  model: string;
  onModel: (v: string) => void;
  onRemove: () => void;
  removable: boolean;
  multimodal: boolean;
}) {
  const { t } = useTranslation();
  const [msgs, setMsgs] = React.useState<Msg[]>([]);
  // rendered by default, because that is what a model reply is *for*; raw is
  // what an operator switches to when the question is what the model literally
  // emitted — a stray delimiter, trailing whitespace, an unclosed fence (#955)
  const [raw, setRaw] = React.useState(false);
  const [draft, setDraft] = React.useState("");
  const [image, setImage] = React.useState<string | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [lastPrompt, setLastPrompt] = React.useState("");
  const fileRef = React.useRef<HTMLInputElement>(null);

  const attach = (f: File) => {
    const reader = new FileReader();
    reader.onload = () => setImage(reader.result as string);
    reader.readAsDataURL(f);
  };

  const send = async () => {
    if (!draft.trim() || busy) return;
    const userText = draft;
    setLastPrompt(userText);
    const attached = image;
    setDraft("");
    setImage(null);
    setError(null);
    setMsgs((m) => [
      ...m,
      { role: "user", text: userText },
      { role: "assistant", text: "…", pending: true },
    ]);
    setBusy(true);

    const content: ChatMessage["content"] = attached
      ? [
          { type: "text", text: userText },
          { type: "image_url", image_url: { url: attached } },
        ]
      : userText;
    try {
      const reply = await chatCompletion(model, [{ role: "user", content }]);
      setMsgs((m) =>
        m.map((msg, i) => (i === m.length - 1 ? { role: "assistant", text: reply } : msg)),
      );
    } catch (e) {
      setMsgs((m) => m.slice(0, -1));
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-[460px] flex-col overflow-hidden rounded-lg border border-[color:var(--border-default)] bg-card">
      <div className="flex items-center gap-2 border-b border-[color:var(--border-subtle)] p-2">
        <ModelSelect models={models} value={model} onChange={onModel} />
        {/* the last thing sent, so the snippet reproduces a call that is known
            to work rather than whatever is half-typed in the composer */}
        <CopyAsCodeButton
          request={{
            model,
            prompt: lastPrompt || draft || "Hello!",
          }}
        />
        <Button
          size="icon"
          variant="ghost"
          className="h-8 w-8"
          aria-pressed={raw}
          onClick={() => setRaw((v) => !v)}
          aria-label={t("playground.rawOutput")}
          title={t("playground.rawOutput")}
        >
          <Pilcrow className="h-3.5 w-3.5" />
        </Button>
        {removable && (
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            onClick={onRemove}
            aria-label={t("pages.playground.removeColumn")}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
      <div className="flex flex-1 flex-col gap-2.5 overflow-auto p-4">
        {msgs.length === 0 && (
          <p className="m-auto text-center text-xs text-muted-foreground">
            {t("pages.playground.sendMessageTo", { model })}
          </p>
        )}
        {msgs.map((m, i) => (
          <div
            key={i}
            className={
              m.role === "user"
                ? "flex flex-col gap-1 rounded-md bg-[color:var(--surface-subtle)] px-2.5 py-2 text-sm text-foreground"
                : "flex flex-col gap-1 rounded-md border border-[color:var(--border-subtle)] px-2.5 py-2 text-sm text-[color:var(--text-secondary)]"
            }
          >
            <span className="font-mono text-[0.625rem] uppercase tracking-wide text-[color:var(--text-subtle)]">
              {m.role}
            </span>
            {/* markdown, unless the operator asked for the characters. the
                renderer takes a partial reply as readily as a finished one, so
                an unterminated fence mid-stream renders as the code block it
                is about to become rather than breaking the column (#955) */}
            {m.pending ? (
              <span className="text-[color:var(--text-muted)]">{m.text}</span>
            ) : raw ? (
              /* ui-primitives-allow: the raw-output toggle, which shows the
               * model's reply as the characters it sent instead of rendered
               * markdown. prose with its newlines kept, not a payload: it wraps
               * rather than scrolling sideways, and `CodeBlock`'s copy button and
               * highlighting would both be answering a question nobody asked */
              <pre className="whitespace-pre-wrap break-words font-mono text-xs">{m.text}</pre>
            ) : (
              <Markdown source={m.text} />
            )}
          </div>
        ))}
      </div>
      {(error || image) && (
        <div className="px-3 pb-1">
          {image && (
            <div className="mb-1 inline-flex items-center gap-1.5 rounded bg-[color:var(--surface-subtle)] px-2 py-1 text-[0.625rem] text-muted-foreground">
              <ImageIcon className="h-3 w-3" /> {t("pages.playground.imageAttached")}
              <button
                onClick={() => setImage(null)}
                aria-label={t("pages.playground.removeAttachment")}
                className="focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
              >
                <Trash2 className="h-3 w-3" />
              </button>
            </div>
          )}
          <ErrorNote error={error} />
        </div>
      )}
      <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] p-2.5">
        {multimodal && (
          <>
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              hidden
              onChange={(e) => e.target.files?.[0] && attach(e.target.files[0])}
            />
            <Button
              size="icon"
              variant="ghost"
              className="h-8 w-8"
              onClick={() => fileRef.current?.click()}
              aria-label={t("pages.playground.attachImage")}
            >
              <Paperclip className="h-4 w-4" />
            </Button>
          </>
        )}
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && send()}
          placeholder={t("pages.playground.messagePlaceholder")}
          className="h-8 flex-1 text-sm"
        />
        <Button
          size="icon"
          className="h-8 w-8"
          onClick={send}
          disabled={busy}
          aria-label={t("pages.playground.send")}
        >
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
        </Button>
      </div>
    </div>
  );
}

function ChatMode({ models }: { models: ModelOption[] }) {
  const { t } = useTranslation();
  const [cols, setCols] = React.useState<{ model: string }[]>([{ model: FAKE }]);
  const [multimodal, setMultimodal] = React.useState(false);
  // the list arrives after the first render. until the operator picks
  // something, the first real route beats the built-in placeholder: a
  // deployment with models configured should not open on lorem ipsum
  const touched = React.useRef(false);
  React.useEffect(() => {
    if (touched.current) return;
    const real = models.find((m) => m.id !== FAKE && !m.id.includes("/"));
    if (real) setCols((c) => (c.length === 1 && c[0].model === FAKE ? [{ model: real.id }] : c));
  }, [models]);
  const compare = cols.length > 1;
  const setModel = (i: number, v: string) => {
    touched.current = true;
    setCols((c) => c.map((col, j) => (j === i ? { model: v } : col)));
  };
  const add = () => setCols((c) => [...c, { model: models[c.length % models.length]?.id ?? FAKE }]);
  const remove = (i: number) => setCols((c) => c.filter((_, j) => j !== i));

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2.5">
        <label className="flex items-center gap-2 text-sm">
          <Switch
            checked={multimodal}
            aria-labelledby="playground-multimodal-label"
            onCheckedChange={setMultimodal}
          />
          <span id="playground-multimodal-label">{t("pages.playground.multimodal")}</span>
        </label>
        <span className="text-xs text-[color:var(--text-subtle)]">
          {t("pages.playground.attachHint")}
        </span>
        <span className="ml-auto flex items-center gap-2">
          <Badge tone={compare ? "accent" : "neutral"}>
            {compare
              ? t("pages.playground.compareBadge", { count: cols.length })
              : t("pages.playground.single")}
          </Badge>
          <Button size="sm" variant="outline" onClick={add}>
            <GitCompare className="h-3.5 w-3.5" /> {t("pages.playground.addModel")}
          </Button>
        </span>
      </div>
      <div
        className="flex gap-4 pb-1.5"
        style={{ overflowX: cols.length > 2 ? "auto" : "visible" }}
      >
        {cols.map((c, i) => (
          <div
            key={i}
            style={{
              minWidth: cols.length > 2 ? 340 : 0,
              flex: cols.length > 2 ? "none" : 1,
              width: cols.length > 2 ? 340 : "auto",
            }}
          >
            <ChatColumn
              models={models}
              model={c.model}
              multimodal={multimodal}
              removable={cols.length > 1}
              onModel={(v) => setModel(i, v)}
              onRemove={() => remove(i)}
            />
          </div>
        ))}
      </div>
    </div>
  );
}

/* ---------------- embeddings (PCA → 2D) ---------------- */
function pca2(vectors: number[][]): { x: number; y: number }[] {
  const n = vectors.length;
  const d = vectors[0].length;
  const mean = Array(d).fill(0);
  vectors.forEach((v) => v.forEach((x, j) => (mean[j] += x / n)));
  const X = vectors.map((v) => v.map((x, j) => x - mean[j]));
  const cov = Array.from({ length: d }, () => Array(d).fill(0));
  X.forEach((v) => {
    for (let a = 0; a < d; a++) for (let b = 0; b < d; b++) cov[a][b] += (v[a] * v[b]) / n;
  });
  const norm = (v: number[]) => {
    const l = Math.hypot(...v) || 1;
    return v.map((x) => x / l);
  };
  const mul = (m: number[][], v: number[]) =>
    m.map((row) => row.reduce((s, x, j) => s + x * v[j], 0));
  const eig = (deflate: number[][]) => {
    let v = norm(Array.from({ length: d }, (_, i) => Math.sin(i + 1)));
    for (let it = 0; it < 60; it++) {
      let w = mul(cov, v);
      deflate.forEach((e) => {
        const dot = e.reduce((s, x, j) => s + x * w[j], 0);
        w = w.map((x, j) => x - dot * e[j]);
      });
      v = norm(w);
    }
    return v;
  };
  const e1 = eig([]);
  const e2 = eig([e1]);
  return X.map((v) => ({
    x: v.reduce((s, x, j) => s + x * e1[j], 0),
    y: v.reduce((s, x, j) => s + x * e2[j], 0),
  }));
}

function EmbeddingsMode({ models }: { models: ModelOption[] }) {
  const { t } = useTranslation();
  const [model, setModel] = React.useState(FAKE);
  // sample input, seeded in the operator's language: one text per line
  const [texts, setTexts] = React.useState<string[]>(() =>
    t("pages.playground.samples.embeddings").split("\n"),
  );
  const [points, setPoints] = React.useState<ScatterPoint[]>([]);
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);

  const setText = (i: number, v: string) => setTexts((a) => a.map((t, j) => (j === i ? v : t)));
  const addField = () => setTexts((a) => [...a, ""]);
  const removeField = (i: number) =>
    setTexts((a) => (a.length > 1 ? a.filter((_, j) => j !== i) : a));

  const run = async () => {
    const rows = texts.filter((t) => t.trim());
    if (rows.length < 2) {
      setError(t("pages.playground.embedNeedTwo"));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const vecs = await embed(model, rows);
      const proj = pca2(vecs);
      const xs = proj.map((p) => p.x);
      const ys = proj.map((p) => p.y);
      const nx = (v: number) =>
        ((v - Math.min(...xs)) / (Math.max(...xs) - Math.min(...xs) || 1)) * 100;
      const ny = (v: number) =>
        ((v - Math.min(...ys)) / (Math.max(...ys) - Math.min(...ys) || 1)) * 100;
      setPoints(proj.map((p, i) => ({ x: nx(p.x), y: ny(p.y), label: rows[i] })));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 lg:grid-cols-[360px_1fr]">
      <div className="flex flex-col">
        <ModelSelect models={models} value={model} onChange={setModel} className="mb-2.5" />
        <div className="flex max-h-[280px] flex-col gap-1.5 overflow-y-auto pr-1">
          {texts.map((row, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="w-4 flex-none text-right font-mono text-[0.625rem] text-[color:var(--text-subtle)]">
                {i + 1}
              </span>
              <Input
                value={row}
                onChange={(e) => setText(i, e.target.value)}
                placeholder={t("pages.playground.textPlaceholder")}
                className="h-8 text-sm"
              />
              <Button
                size="icon"
                variant="ghost"
                className="h-8 w-8"
                onClick={() => removeField(i)}
                aria-label={t("pages.playground.removeText")}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </div>
        <div className="mt-2.5 flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={addField}>
            <Plus className="h-3.5 w-3.5" /> {t("pages.playground.addText")}
          </Button>
          <Button size="sm" onClick={run} disabled={busy}>
            {busy ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}{" "}
            {t("pages.playground.embedProject")}
          </Button>
        </div>
        <ErrorNote error={error} />
      </div>
      <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
        <p className="mb-2.5 font-mono text-xs text-muted-foreground">
          {t("pages.playground.pcaCount", { count: points.length })}
        </p>
        {points.length ? (
          <ScatterPlot
            height={300}
            xLabel="PC1"
            yLabel="PC2"
            label={t("pages.playground.pcaChartAria")}
            points={points}
          />
        ) : (
          <p className="py-16 text-center text-sm text-muted-foreground">
            {t("pages.playground.embedEmpty")}
          </p>
        )}
      </div>
    </div>
  );
}

/* ---------------- image ---------------- */
function ImageMode({ models }: { models: ModelOption[] }) {
  const { t } = useTranslation();
  const [model, setModel] = React.useState(FAKE);
  const [prompt, setPrompt] = React.useState(() => t("pages.playground.samples.imagePrompt"));
  const [size, setSize] = React.useState("1024x1024");
  const [n, setN] = React.useState(4);
  const [images, setImages] = React.useState<GeneratedImage[]>([]);
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);

  const gen = async () => {
    setError(null);
    setBusy(true);
    try {
      setImages(await generateImages(model, prompt, n, size));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 lg:grid-cols-[360px_1fr]">
      <div className="flex flex-col gap-2.5">
        <ModelSelect models={models} value={model} onChange={setModel} />
        <Textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          className="min-h-[120px] text-sm"
        />
        <div className="flex gap-2.5">
          <Combobox
            value={size}
            onChange={(picked) => setSize(picked)}
            aria-label={t("pages.playground.imageSizeAria")}
            className="h-8 text-xs"
            options={[
              { value: "1024x1024", label: "1024²" },
              { value: "1024x1792", label: "1024×1792" },
              { value: "1792x1024", label: "1792×1024" },
            ]}
          />
          <Combobox
            value={String(n)}
            onChange={(picked) => setN(Number(picked))}
            aria-label={t("pages.playground.imageCountAria")}
            className="h-8 text-xs"
            options={[
              { value: "1", label: "n=1" },
              { value: "4", label: "n=4" },
            ]}
          />
          <Button size="sm" onClick={gen} disabled={busy}>
            {busy ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <ImageIcon className="h-3.5 w-3.5" />
            )}{" "}
            {t("pages.playground.generate")}
          </Button>
        </div>
        <ErrorNote error={error} />
      </div>
      <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
        <p className="mb-2.5 font-mono text-xs text-muted-foreground">
          {t("pages.playground.outputCount", { count: images.length })}
        </p>
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
          {(images.length ? images : Array.from({ length: n }, () => null)).map((img, i) => (
            <div
              key={i}
              className="flex aspect-square items-center justify-center overflow-hidden rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)]"
            >
              {img ? (
                <img
                  src={img.url}
                  alt={t("pages.playground.sampleLabel", { n: i + 1 })}
                  className="h-full w-full object-cover"
                />
              ) : (
                <span className="inline-flex items-center gap-1.5 font-mono text-xs text-[color:var(--text-subtle)]">
                  <ImageIcon className="h-4 w-4" />{" "}
                  {t("pages.playground.sampleLabel", { n: i + 1 })}
                </span>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

/* ---------------- audio ---------------- */
function AudioMode({ models }: { models: ModelOption[] }) {
  const { t } = useTranslation();
  const [tab, setTab] = React.useState("tts");
  const [model, setModel] = React.useState(FAKE);
  const [text, setText] = React.useState(() => t("pages.playground.samples.speech"));
  const [voice, setVoice] = React.useState("nova");
  const [audioUrl, setAudioUrl] = React.useState<string | null>(null);
  const [transcript, setTranscript] = React.useState<string | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);
  const fileRef = React.useRef<HTMLInputElement>(null);

  const speak = async () => {
    setError(null);
    setBusy(true);
    try {
      setAudioUrl(await synthesizeSpeech(model, text, voice));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const doTranscribe = async (f: File) => {
    setError(null);
    setBusy(true);
    setTranscript(null);
    try {
      setTranscript(await transcribe(model, f));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { value: "tts", label: t("pages.playground.audioTabs.tts") },
          { value: "stt", label: t("pages.playground.audioTabs.stt") },
        ]}
      />
      {tab === "tts" ? (
        <div className="grid gap-4 lg:grid-cols-[360px_1fr]">
          <div className="flex flex-col gap-2.5">
            <ModelSelect models={models} value={model} onChange={setModel} />
            <Textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              className="min-h-[100px] text-sm"
            />
            <div className="flex gap-2.5">
              <Combobox
                value={voice}
                onChange={(picked) => setVoice(picked)}
                aria-label={t("pages.playground.voiceAria")}
                className="h-8 text-xs"
                options={[
                  { value: "nova", label: t("pages.playground.voiceOption", { voice: "nova" }) },
                  { value: "onyx", label: t("pages.playground.voiceOption", { voice: "onyx" }) },
                  {
                    value: "shimmer",
                    label: t("pages.playground.voiceOption", { voice: "shimmer" }),
                  },
                ]}
              />
              <Button size="sm" onClick={speak} disabled={busy}>
                {busy ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Mic className="h-3.5 w-3.5" />
                )}{" "}
                {t("pages.playground.synthesize")}
              </Button>
            </div>
            <ErrorNote error={error} />
          </div>
          <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
            <p className="mb-2.5 font-mono text-xs text-muted-foreground">
              {t("pages.playground.output")}
            </p>
            {audioUrl ? (
              <audio controls src={audioUrl} className="w-full" />
            ) : (
              <p className="py-10 text-center text-sm text-muted-foreground">
                {t("pages.playground.synthesizeEmpty")}
              </p>
            )}
          </div>
        </div>
      ) : (
        <div className="grid gap-4 lg:grid-cols-[360px_1fr]">
          <div className="flex flex-col gap-2.5">
            <ModelSelect models={models} value={model} onChange={setModel} />
            <input
              ref={fileRef}
              type="file"
              accept="audio/*"
              hidden
              onChange={(e) => e.target.files?.[0] && doTranscribe(e.target.files[0])}
            />
            <Button
              size="sm"
              variant="outline"
              onClick={() => fileRef.current?.click()}
              disabled={busy}
            >
              {busy ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Upload className="h-3.5 w-3.5" />
              )}{" "}
              {t("pages.playground.uploadAudio")}
            </Button>
            <ErrorNote error={error} />
          </div>
          <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
            <p className="mb-2.5 font-mono text-xs text-muted-foreground">
              {t("pages.playground.transcript")}
            </p>
            {transcript != null ? (
              <p className="text-sm text-foreground">
                {transcript || t("pages.playground.transcriptEmpty")}
              </p>
            ) : (
              <p className="py-10 text-center text-sm text-muted-foreground">
                {t("pages.playground.transcribeEmpty")}
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/* ---------------- realtime (WebSocket) ---------------- */
function RealtimeMode({ models }: { models: ModelOption[] }) {
  const { t } = useTranslation();
  const [model, setModel] = React.useState(models[0]?.id ?? FAKE);
  const [live, setLive] = React.useState(false);
  const [log, setLog] = React.useState<string[]>([]);
  const [draft, setDraft] = React.useState("");
  const wsRef = React.useRef<WebSocket | null>(null);

  const append = (line: string) => setLog((l) => [...l.slice(-40), line]);

  const stop = React.useCallback(() => {
    wsRef.current?.close();
    wsRef.current = null;
    setLive(false);
  }, []);

  const start = () => {
    try {
      const ws = new WebSocket(realtimeUrl(model));
      wsRef.current = ws;
      ws.onopen = () => {
        setLive(true);
        append(`● ${t("pages.playground.realtimeLog.connected")}`);
      };
      ws.onmessage = (ev) => append("← " + String(ev.data).slice(0, 200));
      ws.onerror = () => append(`✕ ${t("pages.playground.realtimeLog.socketError")}`);
      ws.onclose = () => {
        setLive(false);
        append(`○ ${t("pages.playground.realtimeLog.closed")}`);
      };
    } catch (e) {
      append("✕ " + (e as Error).message);
    }
  };

  React.useEffect(() => () => wsRef.current?.close(), []);

  const send = () => {
    if (!draft.trim() || !wsRef.current) return;
    wsRef.current.send(JSON.stringify({ type: "input_text", text: draft }));
    append("→ " + draft);
    setDraft("");
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2.5">
        <ModelSelect models={models} value={model} onChange={setModel} />
        <span className="ml-auto">
          <Button
            size="sm"
            variant={live ? "destructive" : "default"}
            onClick={live ? stop : start}
          >
            <Mic className="h-3.5 w-3.5" />{" "}
            {live ? t("pages.playground.stopSession") : t("pages.playground.startSession")}
          </Button>
        </span>
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("pages.playground.sessionTitle")}</CardTitle>
            <CardDescription>{t("pages.playground.sessionSubtitle")}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-1">
            <StatusRow
              status={live ? "success" : "idle"}
              chevron={false}
              label={live ? t("pages.playground.connected") : t("pages.playground.idle")}
            />
            <StatusRow
              status={live ? "running" : "idle"}
              chevron={false}
              label={live ? t("pages.playground.channelOpen") : t("pages.playground.noSession")}
            />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("pages.playground.eventLog")}</CardTitle>
            <CardDescription>{t("pages.playground.eventLogSubtitle")}</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="mb-2.5 flex h-[140px] flex-col gap-1 overflow-auto font-mono text-[0.6875rem] text-[color:var(--text-secondary)]">
              {log.length === 0 ? (
                <span className="text-[color:var(--text-subtle)]">
                  {t("pages.playground.noEvents")}
                </span>
              ) : (
                log.map((l, i) => <div key={i}>{l}</div>)
              )}
            </div>
            <div className="flex items-center gap-2">
              <Input
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && send()}
                placeholder={t("pages.playground.framePlaceholder")}
                className="h-8 text-sm"
                disabled={!live}
              />
              <Button
                size="icon"
                className="h-8 w-8"
                onClick={send}
                disabled={!live}
                aria-label={t("pages.playground.send")}
              >
                <Send className="h-4 w-4" />
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

/* ---------------- page ---------------- */
export default function Playground() {
  const { t } = useTranslation();
  const [mode, setMode] = React.useState("chat");
  const { options: models, source, ready } = useModelCatalog();

  // UX stream (#805); the screen key comes from the enclosing UxScreenProvider.
  // Playground is the screen an evaluator spends the most time in, so its
  // time-to-interactive is the number most worth having (#1730)
  useScreenReady(ready);

  return (
    <div className="flex flex-col gap-5 p-[22px]">
      <SessionKeyBar />
      <ModelSourceNotice source={source} />
      <Tabs
        value={mode}
        onChange={setMode}
        tabs={[
          { value: "chat", label: t("pages.playground.modes.chat") },
          { value: "embeddings", label: t("pages.playground.modes.embeddings") },
          { value: "image", label: t("pages.playground.modes.image") },
          { value: "audio", label: t("pages.playground.modes.audio") },
          { value: "realtime", label: t("pages.playground.modes.realtime") },
        ]}
      />
      {mode === "chat" && <ChatMode models={models} />}
      {mode === "embeddings" && <EmbeddingsMode models={models} />}
      {mode === "image" && <ImageMode models={models} />}
      {mode === "audio" && <AudioMode models={models} />}
      {mode === "realtime" && <RealtimeMode models={models} />}
    </div>
  );
}
