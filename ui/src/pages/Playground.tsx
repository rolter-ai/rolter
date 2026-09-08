import { useQuery } from "@tanstack/react-query";
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
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { CopyAsCodeButton } from "@/components/CodeSnippetDialog";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScatterPlot, type ScatterPoint } from "@/components/ui/scatter-plot";
import { Select } from "@/components/ui/select";
import { StatusRow } from "@/components/ui/status-row";
import { Switch } from "@/components/ui/switch";
import { Tabs } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { fetchModels } from "@/lib/api";
import {
  chatCompletion,
  embed,
  fetchGatewayModels,
  generateImages,
  getPlaygroundKey,
  realtimeUrl,
  setPlaygroundKey,
  synthesizeSpeech,
  transcribe,
  type ChatMessage,
  type GeneratedImage,
} from "@/lib/gateway";

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
function useModelCatalog(): { options: ModelOption[]; source: ModelSource } {
  const key = getPlaygroundKey();
  const routes = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  const gateway = useQuery({
    queryKey: ["gateway-models", key],
    queryFn: ({ signal }) => fetchGatewayModels(signal),
    enabled: !!key,
    retry: false,
  });

  const source: ModelSource = gateway.data
    ? "gateway"
    : key
      ? "unreachable"
      : "no-key";

  const options: ModelOption[] = gateway.data
    ? gateway.data.map((m) => ({ id: m.id, ownedBy: m.owned_by }))
    : (routes.data ?? []).map((m) => ({ id: m.model }));

  // the built-in always works, so it stays selectable whatever the source
  const withFake = options.some((o) => o.id === FAKE)
    ? options
    : [{ id: FAKE, ownedBy: "rolter" }, ...options];
  return { options: withFake, source };
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
  // are grouped by owner — `owned_by` already carries what is needed (#946)
  const groups = new Map<string, ModelOption[]>();
  for (const option of models) {
    const label = groupLabel(option, routesLabel);
    const bucket = groups.get(label);
    if (bucket) bucket.push(option);
    else groups.set(label, [option]);
  }

  return (
    <Select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={t("pages.playground.modelAria")}
      className={className ?? "h-8 text-xs"}
    >
      {[...groups].map(([label, options]) => (
        <optgroup key={label} label={label}>
          {options.map((m) => (
            <option key={m.id} value={m.id}>
              {m.id}
            </option>
          ))}
        </optgroup>
      ))}
    </Select>
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

/* ---------------- virtual key bar ---------------- */
function KeyBar() {
  const { t } = useTranslation();
  const [key, setKey] = React.useState(getPlaygroundKey());
  const [saved, setSaved] = React.useState(false);
  const save = () => {
    setPlaygroundKey(key.trim());
    setSaved(true);
    setTimeout(() => setSaved(false), 1400);
  };
  return (
    <div className="rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2">
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
        />
        <Button size="sm" variant="outline" onClick={save}>
          {saved ? t("playground.key.saved") : t("playground.key.save")}
        </Button>
      </div>
      {/* three different credentials in this product answer to "api key";
          say which one this field wants (#943) */}
      <p className="mt-1.5 text-[0.6875rem] leading-snug text-[color:var(--text-subtle)]">
        {t("playground.key.hint")}
      </p>
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
        m.map((msg, i) =>
          i === m.length - 1 ? { role: "assistant", text: reply } : msg,
        ),
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
          <Button size="icon" variant="ghost" className="h-8 w-8" onClick={onRemove} aria-label="Remove column">
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
      <div className="flex flex-1 flex-col gap-2.5 overflow-auto p-4">
        {msgs.length === 0 && (
          <p className="m-auto text-center text-xs text-muted-foreground">
            Send a message to {model}.
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
              <ImageIcon className="h-3 w-3" /> image attached
              <button onClick={() => setImage(null)} aria-label="Remove attachment" className="focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded">
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
              aria-label="Attach image"
            >
              <Paperclip className="h-4 w-4" />
            </Button>
          </>
        )}
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && send()}
          placeholder="Message…"
          className="h-8 flex-1 text-sm"
        />
        <Button size="icon" className="h-8 w-8" onClick={send} disabled={busy} aria-label="Send">
          <Send className="h-4 w-4" />
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
  const add = () =>
    setCols((c) => [...c, { model: models[c.length % models.length]?.id ?? FAKE }]);
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
          · attach images to any turn
        </span>
        <span className="ml-auto flex items-center gap-2">
          <Badge tone={compare ? "accent" : "neutral"}>
            {compare ? `Compare · ${cols.length}` : "Single"}
          </Badge>
          <Button size="sm" variant="outline" onClick={add}>
            <GitCompare className="h-3.5 w-3.5" /> Add model
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
  const [texts, setTexts] = React.useState<string[]>([
    "reset my password",
    "update the invoice",
    "read the api docs",
    "two-factor auth setup",
    "monthly billing statement",
    "getting started guide",
  ]);
  const [points, setPoints] = React.useState<ScatterPoint[]>([]);
  const [error, setError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState(false);

  const setText = (i: number, v: string) =>
    setTexts((a) => a.map((t, j) => (j === i ? v : t)));
  const addField = () => setTexts((a) => [...a, ""]);
  const removeField = (i: number) =>
    setTexts((a) => (a.length > 1 ? a.filter((_, j) => j !== i) : a));

  const run = async () => {
    const rows = texts.filter((t) => t.trim());
    if (rows.length < 2) {
      setError("enter at least two texts to project");
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
        ((v - Math.min(...xs)) / ((Math.max(...xs) - Math.min(...xs)) || 1)) * 100;
      const ny = (v: number) =>
        ((v - Math.min(...ys)) / ((Math.max(...ys) - Math.min(...ys)) || 1)) * 100;
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
        <ModelSelect models={models} value={model} onChange={setModel} className="mb-2.5 h-8 text-xs" />
        <div className="flex max-h-[280px] flex-col gap-1.5 overflow-y-auto pr-1">
          {texts.map((t, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="w-4 flex-none text-right font-mono text-[0.625rem] text-[color:var(--text-subtle)]">
                {i + 1}
              </span>
              <Input
                value={t}
                onChange={(e) => setText(i, e.target.value)}
                placeholder="Enter text…"
                className="h-8 text-sm"
              />
              <Button
                size="icon"
                variant="ghost"
                className="h-8 w-8"
                onClick={() => removeField(i)}
                aria-label="Remove text"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </div>
        <div className="mt-2.5 flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={addField}>
            <Plus className="h-3.5 w-3.5" /> Add text
          </Button>
          <Button size="sm" onClick={run} disabled={busy}>
            <Play className="h-3.5 w-3.5" /> Embed &amp; project
          </Button>
        </div>
        <ErrorNote error={error} />
      </div>
      <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
        <p className="mb-2.5 font-mono text-xs text-muted-foreground">
          PCA projection · {points.length} vectors
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
            Embed some texts to see them projected to 2D.
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
  const [prompt, setPrompt] = React.useState(
    "A cross-stitch folk pattern of a fox, deep red thread on black linen",
  );
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
          <Select
            value={size}
            onChange={(e) => setSize(e.target.value)}
            aria-label={t("pages.playground.imageSizeAria")}
            className="h-8 text-xs"
          >
            <option value="1024x1024">1024²</option>
            <option value="1024x1792">1024×1792</option>
            <option value="1792x1024">1792×1024</option>
          </Select>
          <Select
            value={String(n)}
            onChange={(e) => setN(Number(e.target.value))}
            aria-label={t("pages.playground.imageCountAria")}
            className="h-8 text-xs"
          >
            <option value="1">n=1</option>
            <option value="4">n=4</option>
          </Select>
          <Button size="sm" onClick={gen} disabled={busy}>
            <ImageIcon className="h-3.5 w-3.5" /> Generate
          </Button>
        </div>
        <ErrorNote error={error} />
      </div>
      <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
        <p className="mb-2.5 font-mono text-xs text-muted-foreground">
          Output · {images.length} sample{images.length === 1 ? "" : "s"}
        </p>
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
          {(images.length ? images : Array.from({ length: n }, () => null)).map(
            (img, i) => (
              <div
                key={i}
                className="flex aspect-square items-center justify-center overflow-hidden rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)]"
              >
                {img ? (
                  <img src={img.url} alt={`sample ${i + 1}`} className="h-full w-full object-cover" />
                ) : (
                  <span className="inline-flex items-center gap-1.5 font-mono text-xs text-[color:var(--text-subtle)]">
                    <ImageIcon className="h-4 w-4" /> sample {i + 1}
                  </span>
                )}
              </div>
            ),
          )}
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
  const [text, setText] = React.useState(
    "Привет! This is a synthesized voice sample from the Rolter playground.",
  );
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
          { value: "tts", label: "Text → Speech" },
          { value: "stt", label: "Speech → Text" },
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
              <Select
                value={voice}
                onChange={(e) => setVoice(e.target.value)}
                aria-label={t("pages.playground.voiceAria")}
                className="h-8 text-xs"
              >
                <option value="nova">voice: nova</option>
                <option value="onyx">voice: onyx</option>
                <option value="shimmer">voice: shimmer</option>
              </Select>
              <Button size="sm" onClick={speak} disabled={busy}>
                <Mic className="h-3.5 w-3.5" /> Synthesize
              </Button>
            </div>
            <ErrorNote error={error} />
          </div>
          <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
            <p className="mb-2.5 font-mono text-xs text-muted-foreground">Output</p>
            {audioUrl ? (
              <audio controls src={audioUrl} className="w-full" />
            ) : (
              <p className="py-10 text-center text-sm text-muted-foreground">
                Synthesize to hear the result.
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
            <Button size="sm" variant="outline" onClick={() => fileRef.current?.click()} disabled={busy}>
              <Upload className="h-3.5 w-3.5" /> Upload audio
            </Button>
            <ErrorNote error={error} />
          </div>
          <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
            <p className="mb-2.5 font-mono text-xs text-muted-foreground">Transcript</p>
            {transcript != null ? (
              <p className="text-sm text-foreground">{transcript || "(empty)"}</p>
            ) : (
              <p className="py-10 text-center text-sm text-muted-foreground">
                Upload an audio file to transcribe.
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
        append("● connected");
      };
      ws.onmessage = (ev) => append("← " + String(ev.data).slice(0, 200));
      ws.onerror = () => append("✕ socket error — realtime needs a realtime-capable upstream");
      ws.onclose = () => {
        setLive(false);
        append("○ closed");
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
            <Mic className="h-3.5 w-3.5" /> {live ? "Stop session" : "Start session"}
          </Button>
        </span>
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Session</CardTitle>
            <CardDescription>WebSocket · bidirectional audio + text</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-1">
            <StatusRow
              status={live ? "success" : "idle"}
              chevron={false}
              label={live ? "Connected" : "Idle"}
            />
            <StatusRow
              status={live ? "running" : "idle"}
              chevron={false}
              label={live ? "Channel open" : "No session"}
            />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Event log</CardTitle>
            <CardDescription>Frames sent and received</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="mb-2.5 flex h-[140px] flex-col gap-1 overflow-auto font-mono text-[0.6875rem] text-[color:var(--text-secondary)]">
              {log.length === 0 ? (
                <span className="text-[color:var(--text-subtle)]">no events yet</span>
              ) : (
                log.map((l, i) => <div key={i}>{l}</div>)
              )}
            </div>
            <div className="flex items-center gap-2">
              <Input
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && send()}
                placeholder="Send a text frame…"
                className="h-8 text-sm"
                disabled={!live}
              />
              <Button size="icon" className="h-8 w-8" onClick={send} disabled={!live} aria-label="Send">
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
  const [mode, setMode] = React.useState("chat");
  const { options: models, source } = useModelCatalog();

  return (
    <div className="flex flex-col gap-5 p-[22px]">
      <KeyBar />
      <ModelSourceNotice source={source} />
      <Tabs
        value={mode}
        onChange={setMode}
        tabs={[
          { value: "chat", label: "Chat" },
          { value: "embeddings", label: "Embeddings" },
          { value: "image", label: "Image" },
          { value: "audio", label: "Audio" },
          { value: "realtime", label: "Realtime" },
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
