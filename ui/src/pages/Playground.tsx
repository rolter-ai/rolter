import { useMutation, useQuery, type UseMutationResult } from "@tanstack/react-query";
import {
  Check,
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
import { Link } from "react-router";

import { CopyAsCodeButton } from "@/components/CodeSnippetDialog";
import { DocsLink } from "@/components/DocsLink";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { Markdown } from "@/components/Markdown";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button, type ButtonProps } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Input } from "@/components/ui/input";
import { ScatterPlot, type ScatterPoint } from "@/components/ui/scatter-plot";
import { StatusRow } from "@/components/ui/status-row";
import { Switch } from "@/components/ui/switch";
import { Tabs } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  ApiError,
  fetchConfigProblems,
  fetchModels,
  mintPlaygroundKey,
  unservedRoutes,
  type MintedKey,
} from "@/lib/api";
import { useCan, useCapabilities } from "@/lib/can";
import {
  awaitingMintedKey,
  chatCompletion,
  embed,
  fetchGatewayModels,
  generateImages,
  getPlaygroundKeyState,
  isKeyRefusal,
  keyPropagationDelay,
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
import { useOptionalPreferences } from "@/lib/preferences";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";
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
 * other is what #946 is about. `waiting` is the stretch between minting a key
 * and the gateway accepting it, which is neither a gateway list nor a failure
 * (#1853). `asking` is the one keyless call that finds out whether the gateway
 * wants a key at all (#2061).
 */
export type ModelSource = "gateway" | "waiting" | "asking" | "no-key" | "unreachable";

/** What the model pickers offer, and what the chat column should open on. */
interface ModelCatalog {
  options: ModelOption[];
  source: ModelSource;
  ready: boolean;
  /**
   * The model to select before the operator picks one, or `null` while the
   * list could still change or could still hold a route the gateway does not
   * serve.
   */
  preferred: string | null;
  /** configured routes left out of a fallback list because they are not served */
  hidden: number;
  /** the gateway turned down the key in hand, rather than failing for its own reasons */
  rejected: boolean;
  /** there is no key, and the gateway answered without one */
  keyless: boolean;
}

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
 *
 * The route list is the store's, so it also holds routes the control plane
 * prunes from the gateway's snapshot. Those are left out of the fallback, and
 * nothing is preselected from it until `/api/v1/config/problems` has said
 * which ones they are: opening on a route the gateway does not serve made the
 * first message an operator sent fail with "no route" (#1853).
 *
 * With no key and none on its way, the gateway is asked once without one. A
 * gateway no control plane manages, holding no keys, serves anybody
 * (`authenticate` in crates/rolter-gateway/src/handlers.rs) — the
 * no-database `rolter easy-up` — and there the screen works with no key at
 * all. While a mint is due that call could only be refused, so it waits
 * (#2061).
 */
function useModelCatalog(session: KeySession): ModelCatalog {
  // the key is read through the store rather than once at render, so the list
  // re-fetches the moment the screen mints one (#944)
  const { key, minted } = session.state;
  // a mint in flight is a key about to arrive, not a screen without one
  const minting = session.pending;
  const keyless = !key && !minting;
  const routes = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  // the same query the Providers screen lists, so the two share one answer
  const problems = useQuery({ queryKey: ["config-problems"], queryFn: fetchConfigProblems });
  const gateway = useQuery({
    queryKey: ["gateway-models", key],
    queryFn: ({ signal }) => fetchGatewayModels(signal),
    enabled: !!key || keyless,
    // a key minted a moment ago answers 401 until the gateway's next snapshot
    // poll picks it up, so that refusal is waited out, with backoff and a
    // bound, before the screen falls back (#1853). a pasted key, or none, gets
    // a single attempt: nothing about it is on its way
    retry: minted ? awaitingMintedKey : false,
    retryDelay: keyPropagationDelay,
  });

  const source: ModelSource = gateway.data
    ? "gateway"
    : key
      ? gateway.isPending
        ? "waiting"
        : "unreachable"
      : minting
        ? "waiting"
        : gateway.isPending
          ? "asking"
          : "no-key";

  // one entry per public name: several projects can route the same model
  const configured = [...new Set((routes.data ?? []).map((m) => m.model))];
  const unserved = unservedRoutes(problems.data);
  const served = configured.filter((model) => !unserved.has(model));

  const options: ModelOption[] = gateway.data
    ? gateway.data.map((m) => ({ id: m.id, ownedBy: m.owned_by }))
    : served.map((model) => ({ id: model }));

  // the built-in always works, so it stays selectable whatever the source
  const withFake = options.some((o) => o.id === FAKE)
    ? options
    : [{ id: FAKE, ownedBy: "rolter" }, ...options];

  // the gateway's own list is served by definition, and its first bare id is
  // a route rather than a provider pin. the fallback only earns a pick once the
  // problems are in, since until then any route in it may be a dead one
  const preferred =
    source === "gateway"
      ? (options.find((o) => o.id !== FAKE && !o.id.includes("/"))?.id ?? null)
      : source !== "waiting" && problems.isSuccess
        ? (served.find((model) => model !== FAKE) ?? null)
        : null;

  // the catalog is what the screen waits on before anything can be sent, so
  // it is the query `time_to_interactive` should be measured against. the
  // gateway probe only counts while it is being made — an `enabled: false`
  // query stays pending forever and would suppress the event
  const ready = !routes.isPending && (!(key || keyless) || !gateway.isPending);
  return {
    options: withFake,
    source,
    ready,
    preferred,
    hidden: gateway.data ? 0 : configured.length - served.length,
    rejected: !!key && gateway.isError && isKeyRefusal(gateway.error),
    keyless: !key && !!gateway.data,
  };
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
 *
 * While a new key is on its way to the gateway the notice says that instead,
 * so the few seconds of waiting do not read as a failure; and a fallback that
 * left out routes the gateway does not serve says how many (#1853).
 */
function ModelSourceNotice({ source, hidden }: { source: ModelSource; hidden: number }) {
  const { t } = useTranslation();
  // while the gateway is being asked whether it wants a key there is nothing
  // to say yet, and a notice that flashes up and away says it badly
  if (source === "gateway" || source === "asking") return null;
  const message =
    source === "waiting"
      ? t("pages.playground.modelsWaiting")
      : source === "no-key"
        ? t("pages.playground.modelsNeedKey")
        : t("pages.playground.modelsUnreachable");
  return (
    <p
      role="status"
      className="rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3 py-2 text-xs text-muted-foreground"
    >
      {message}
      {source !== "waiting" &&
        hidden > 0 &&
        ` ${t("pages.playground.unservedHidden", { count: hidden })}`}
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
 * Whether a minted key has run out, flipped by a timer at the instant it does.
 *
 * Kept apart from `useNow` so the screen as a whole re-renders once, at the
 * expiry, rather than on every tick of the band's countdown.
 */
function useExpired(expiresAt: string | null): boolean {
  const at = expiresAt === null ? null : new Date(expiresAt).getTime();
  const [, wake] = React.useReducer((n: number) => n + 1, 0);
  React.useEffect(() => {
    if (at === null) return;
    const left = at - Date.now();
    if (left <= 0) return;
    const id = setTimeout(wake, left);
    return () => clearTimeout(id);
  }, [at]);
  return at !== null && at <= Date.now();
}

/**
 * Whether a mint was refused because the project routes nothing.
 *
 * The mint takes no body, so the one client error it answers is that one:
 * `mint_playground_key` in crates/rolter-control/src/me.rs returns `400`
 * exactly when the project has no routes, and `control_integration.rs` pins
 * the status. The message is prose and free to be reworded, so it is not read
 * (#2061).
 */
function isRouteless(error: unknown): boolean {
  return error instanceof ApiError && error.status === 400;
}

/**
 * What the screen knows about the key it sends, and whether it can get one
 * itself. One hook, because the key band, the model catalog and every Send on
 * the screen have to agree on it.
 */
interface KeySession {
  state: PlaygroundKeyState;
  /** a minted key past its expiry */
  expired: boolean;
  projectId: string | null;
  /** a key is on its way: the scope, the role check or the mint is still out */
  pending: boolean;
  /** the caller's role is explicitly refused `my_virtual_key:create` here */
  refused: boolean;
  /** a project is in scope and the role is not refused, so a mint can be asked for */
  canMint: boolean;
  mint: UseMutationResult<MintedKey, Error, void>;
  /** the last mint was refused because the project routes nothing */
  routeless: boolean;
}

/**
 * Mints the Playground's key on arrival, once per project.
 *
 * Opening the screen as a signed-in operator mints a key scoped by the control
 * plane to the routes of the project in scope, so the five-second smoke test
 * this screen exists for does not start with a trip to the Keys screen and a
 * paste.
 *
 * Minting is `my_virtual_key:create`, which takes the member role (#2061). The
 * automatic mint waits for the capability answer and does not go out on an
 * explicit "no", so a viewer lands on the paste field rather than on a
 * refusal. An unanswered gate falls open, as everywhere else, and the `403`
 * stays the backstop.
 */
function useKeySession(): KeySession {
  const scope = useScope();
  const state = usePlaygroundKeyState();
  const expired = useExpired(state.minted ? state.expiresAt : null);
  const projectId = scope.projectId ?? null;
  const capabilities = useCapabilities();
  const can = useCan();
  const refused = can("my_virtual_key", "create") === false;
  // with no provider above (a story, a test) there is no answer to wait for
  const gateSettled = !capabilities || capabilities.resolved;

  const mint = useMutation({
    mutationFn: () => mintPlaygroundKey(projectId as string),
    onSuccess: (minted) =>
      setPlaygroundKey(minted.key, { expiresAt: minted.expires_at ?? null, minted: true }),
  });

  // one automatic attempt per project, not one per render: a refusal — a
  // project with no routes answers 400 — must not turn into a mint loop, and
  // the operator mints by hand from here on
  const { mutate } = mint;
  const asked = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!projectId || state.key || !gateSettled || refused || asked.current === projectId) return;
    asked.current = projectId;
    mutate();
  }, [projectId, state.key, gateSettled, refused, mutate]);

  // the automatic mint is due and has not gone out yet, which is a key on its
  // way rather than a screen without one
  const due = !!projectId && !state.key && !refused && asked.current !== projectId;

  return {
    state,
    expired,
    projectId,
    // a scope still resolving only means a key is coming when there is none
    pending: mint.isPending || (!state.key && scope.isLoading) || due,
    refused,
    canMint: !!projectId && !refused,
    mint,
    routeless: isRouteless(mint.error),
  };
}

/**
 * The one thing the key band says, picked by what the screen is waiting on.
 *
 * The band used to stack its lines — the no-project hint, the minted-key hint
 * for a key that did not exist, and the fallback notice — so a first visit
 * read three competing instructions (#2061). Each state now has exactly one.
 * `pending` and `failed` say nothing in the band: the skeleton and the
 * `LoadError` below it carry those.
 */
type KeyMessage =
  | "pending"
  | "failed"
  | "routeless"
  | "expired"
  | "rejected"
  | "active"
  | "pasted"
  | "keyless"
  | "noProject"
  | "refused"
  | "idle";

function keyMessage(
  session: KeySession,
  gateway: { rejected: boolean; keyless: boolean },
): KeyMessage {
  const { state } = session;
  if (session.pending) return "pending";
  if (session.mint.error) return session.routeless ? "routeless" : "failed";
  if (state.key) {
    if (state.minted && session.expired) return "expired";
    if (gateway.rejected) return "rejected";
    return state.minted ? "active" : "pasted";
  }
  if (gateway.keyless) return "keyless";
  if (!session.projectId) return "noProject";
  if (session.refused) return "refused";
  return "idle";
}

/** the states in which the screen could not get a key itself, so pasting one is the way on */
const OFFERS_PASTE: ReadonlySet<KeyMessage> = new Set([
  "failed",
  "routeless",
  "rejected",
  "noProject",
  "refused",
]);

/**
 * The key band: what the screen is sending, and the one thing to know about it.
 *
 * The secret is never rendered: the key is held in memory and shown only as
 * its state, because there is nothing an operator does with the string that
 * the screen is not already doing for them.
 *
 * The paste field stays, for testing one specific key on purpose — the case
 * automatic minting cannot serve — and it opens on its own whenever the
 * screen could not get a key itself.
 */
function SessionKeyBar({
  session,
  rejected,
  keyless,
}: {
  session: KeySession;
  rejected: boolean;
  keyless: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const now = useNow();
  const can = useCan();
  const { state, expired, mint, pending, projectId } = session;
  const message = keyMessage(session, { rejected, keyless });

  const text = (() => {
    switch (message) {
      case "active":
        return t("playground.key.mintedHint");
      case "expired":
        return t("playground.key.expiredHint");
      case "rejected":
        return session.canMint
          ? t("playground.key.rejectedHint")
          : t("playground.key.rejectedPasteHint");
      case "pasted":
        return t("playground.key.pastedHint");
      case "keyless":
        return t("playground.key.keyless");
      case "noProject":
        return t("playground.key.noProject");
      case "refused":
        return t("playground.key.refused");
      case "idle":
        return t("playground.key.idle");
      case "routeless":
        return t("playground.key.routeless");
      default:
        return null;
    }
  })();

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
          {pending ? (
            <ControlSkeleton width={132} />
          ) : (
            <KeyStatus state={state} expired={expired} rejected={rejected} />
          )}
          {!pending && message === "active" && state.expiresAt && (
            <span className="text-xs text-[color:var(--text-subtle)]">
              {t("playground.key.expires", { when: fmt.relative(state.expiresAt, now) })}
            </span>
          )}
          <GatedButton
            gate="my_virtual_key:create"
            control="playground-key-mint"
            size="sm"
            variant="outline"
            className="ml-auto"
            disabled={!projectId || mint.isPending}
            onClick={() => mint.mutate()}
          >
            {mint.isPending && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
            {/* nothing to renew until rolter has minted one: a pasted key is
                replaced, not renewed */}
            {state.key && state.minted ? t("playground.key.renew") : t("playground.key.mint")}
          </GatedButton>
        </div>
        {text && (
          <p role="status" className="text-xs leading-snug text-[color:var(--text-subtle)]">
            {text}
            {/* the fix for a routeless project is a route, so the band points
                at the screen that makes one. only an explicit "no" on reading
                routes hides it, the rule the rail follows for that leaf */}
            {message === "routeless" && can("route", "read") !== false && (
              <>
                {" "}
                <Link
                  to="/routing-rules"
                  className="rounded-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  {t("playground.key.routelessLink", { screen: t("nav.routing-rules") })}
                </Link>
              </>
            )}
          </p>
        )}
        <ManualKeyField
          offered={(state.key !== "" && !state.minted) || OFFERS_PASTE.has(message)}
          // a pasted key supersedes whatever the last mint said
          onSaved={() => mint.reset()}
        />
      </div>
      {/* a routeless project has its own line above and no retry: minting
          again cannot succeed until the project routes something */}
      {message === "failed" && (
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
function KeyStatus({
  state,
  expired,
  rejected,
}: {
  state: PlaygroundKeyState;
  expired: boolean;
  rejected: boolean;
}) {
  const { t } = useTranslation();
  if (!state.key) return <Badge tone="neutral">{t("playground.key.none")}</Badge>;
  if (state.minted && expired)
    return (
      <Badge tone="warning" dot>
        {t("playground.key.expired")}
      </Badge>
    );
  // a key the gateway turned down is not active, whoever chose it (#2061)
  if (rejected)
    return (
      <Badge tone="warning" dot>
        {t("playground.key.rejected")}
      </Badge>
    );
  return state.minted ? (
    <Badge tone="success" dot>
      {t("playground.key.active")}
    </Badge>
  ) : (
    <Badge tone="info" dot>
      {t("playground.key.pasted")}
    </Badge>
  );
}

/**
 * The manual paste field, collapsed unless the screen needs it.
 *
 * Kept because testing one particular key — a customer's, a key that is about
 * to expire — is a real thing to do here, and automatic minting cannot do it.
 * Collapsed while the screen arrives with a key; `offered` opens it when it
 * could not get one, and an operator's own toggle wins over either.
 */
function ManualKeyField({ offered, onSaved }: { offered: boolean; onSaved: () => void }) {
  const { t } = useTranslation();
  const [toggled, setToggled] = React.useState<boolean | null>(null);
  const open = toggled ?? offered;
  const [key, setKey] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const save = () => {
    setPlaygroundKey(key.trim());
    onSaved();
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
        onClick={() => setToggled(!open)}
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
              {saved && <Check className="h-3.5 w-3.5" />}
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

/**
 * Whether the screen holds a key the gateway will take, and if not, why not.
 *
 * Every button that sends through the gateway reads this, because sending
 * without one only bought a `401` on the first message (#2061).
 */
interface SendGate {
  blocked: boolean;
  /** the title a held-back button carries */
  reason?: string;
  /** a key is on its way, rather than missing */
  waiting: boolean;
}

const SendGateContext = React.createContext<SendGate>({ blocked: false, waiting: false });

function useSendGate(session: KeySession, catalog: ModelCatalog): SendGate {
  const { t } = useTranslation();
  const { state, expired, pending } = session;
  // a key in hand is held back only once the gateway refused it or is still
  // learning it. a gateway that fails for its own reasons says so in the
  // column, in its own words, which is more than a disabled button would
  const usable = state.key
    ? !(state.minted && expired) && !catalog.rejected && catalog.source !== "waiting"
    : catalog.keyless;
  const waiting =
    !usable && (pending || catalog.source === "waiting" || catalog.source === "asking");
  const reason = usable
    ? undefined
    : waiting
      ? t("pages.playground.sendWaiting")
      : t("pages.playground.sendNeedsKey");
  return React.useMemo(() => ({ blocked: !usable, reason, waiting }), [usable, reason, waiting]);
}

/**
 * A button that sends a request through the gateway, held back while the
 * screen has no key the gateway will take (#2061).
 *
 * A real `disabled`, with the reason in the `title` the way a refused
 * `GatedButton` carries its role: the button variants drop pointer events when
 * disabled, which would hide the tooltip too, so they come back through an
 * inline style, and `disabled` still swallows the click. `hold` is off for the
 * one state of a button that sends nothing, such as stopping a live session.
 */
function GatewayButton({
  hold = true,
  disabled,
  title,
  style,
  className,
  ...props
}: ButtonProps & { hold?: boolean }) {
  const gate = React.useContext(SendGateContext);
  const held = hold && gate.blocked;
  return (
    <Button
      {...props}
      className={cn(held && "cursor-not-allowed", className)}
      style={held ? { ...style, pointerEvents: "auto" } : style}
      disabled={disabled || held}
      title={held ? gate.reason : title}
    />
  );
}

// an error arrives after an action, so it is announced the moment it appears
function ErrorNote({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p
      role="alert"
      className="rounded-md border border-[color:var(--status-danger)]/40 bg-destructive/10 px-3 py-2 text-xs text-[color:var(--status-danger-text)]"
    >
      {error}
    </p>
  );
}

/**
 * Whether a keydown is the Enter that sends.
 *
 * Shift+Enter is left alone so a multi-line box can take a newline. The Enter
 * that confirms an IME candidate belongs to the composition, not to the send:
 * `isComposing` says so in most browsers, and Safari reports that Enter after
 * the composition has ended, with the legacy keyCode 229.
 */
function isSendKey(e: React.KeyboardEvent): boolean {
  return e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229;
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
  position,
  multimodal,
}: {
  models: ModelOption[];
  model: string;
  onModel: (v: string) => void;
  onRemove: () => void;
  /** 1-based place among the compared columns; `null` for the only column, which cannot be removed */
  position: number | null;
  multimodal: boolean;
}) {
  const { t } = useTranslation();
  // what a control in this column calls the column. the model alone is not
  // enough once two columns can hold the same one, so a compare view adds the
  // place; a lone column is just its model
  const who = position === null ? model : t("pages.playground.columnName", { model, n: position });
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
  // the finished reply, for the live region: `id` makes a reply identical to
  // the last one a new node, so it is announced again
  const [announced, setAnnounced] = React.useState<{ id: number; text: string } | null>(null);
  const fileRef = React.useRef<HTMLInputElement>(null);
  const composer = React.useRef<HTMLTextAreaElement>(null);
  const gate = React.useContext(SendGateContext);

  // the composer grows with the draft up to its max height, then scrolls. a
  // column under another mode tab measures 0 and is left as it was
  React.useLayoutEffect(() => {
    const el = composer.current;
    if (!el || el.offsetParent === null) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
  }, [draft]);

  const attach = (f: File) => {
    const reader = new FileReader();
    reader.onload = () => setImage(reader.result as string);
    reader.readAsDataURL(f);
  };

  const send = async () => {
    // Enter reaches here without the button, so the gate is checked here too
    if (!draft.trim() || busy || gate.blocked) return;
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
      setAnnounced((a) => ({
        id: (a?.id ?? 0) + 1,
        text: t("pages.playground.replyAnnounce", { model, reply }),
      }));
    } catch (e) {
      setMsgs((m) => m.slice(0, -1));
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="relative flex h-[460px] flex-col overflow-hidden rounded-lg border border-[color:var(--border-default)] bg-card">
      <div className="flex items-center gap-2 border-b border-[color:var(--border-subtle)] p-2">
        <ModelSelect models={models} value={model} onChange={onModel} />
        {/* the last thing sent, so the snippet reproduces a call that is known
            to work rather than whatever is half-typed in the composer */}
        <CopyAsCodeButton
          request={{
            model,
            prompt: lastPrompt || draft || "Hello!",
          }}
          label={t("pages.playground.copyAsCodeFor", { model: who })}
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
        {position !== null && (
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            onClick={onRemove}
            aria-label={t("pages.playground.removeColumn", { n: position, model })}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
      {/* a reply taller than the column scrolls here, and a scroller the keyboard
          cannot reach leaves the rest of it to a mouse: a prose reply holds no
          focusable child to carry the focus in. a region named for its column
          makes it one tab stop that says what it is */}
      <div
        tabIndex={0}
        role="region"
        aria-label={t("pages.playground.threadAria", { model: who })}
        className="flex flex-1 flex-col gap-2.5 overflow-auto p-4 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
      >
        {msgs.length === 0 && (
          <p className="m-auto text-center text-xs text-muted-foreground">
            {/* the invitation waits for a key, rather than inviting a message
                the gateway is going to refuse */}
            {gate.blocked && !gate.waiting
              ? t("pages.playground.sendMessageNeedsKey", { model })
              : t("pages.playground.sendMessageTo", { model })}
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
              {t(`pages.playground.roles.${m.role}`)}
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
      {/* the reply is read out once it is whole, not as it arrives: the thread
          is not the live region, so neither the message just typed nor the
          placeholder that holds the reply's place is announced */}
      <div aria-live="polite" aria-atomic="true" className="sr-only">
        {announced && <p key={announced.id}>{announced.text}</p>}
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
      <div className="flex items-end gap-2 border-t border-[color:var(--border-subtle)] p-2.5">
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
        <Textarea
          ref={composer}
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (!isSendKey(e)) return;
            // held back or empty, Enter still must not land a newline in the box
            e.preventDefault();
            void send();
          }}
          placeholder={t("pages.playground.messagePlaceholder")}
          aria-label={t("pages.playground.messageAria", { model })}
          className="max-h-32 min-h-8 flex-1 resize-none py-1 text-sm"
        />
        <GatewayButton
          size="icon"
          className="h-8 w-8"
          onClick={send}
          disabled={busy}
          aria-label={t("pages.playground.sendTo", { model: who })}
        >
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
        </GatewayButton>
      </div>
    </div>
  );
}

function ChatMode({ models, preferred }: { models: ModelOption[]; preferred: string | null }) {
  const { t } = useTranslation();
  // the account's saved Playground model (#2448) is what the column opens on,
  // once the catalog confirms the gateway serves it; a stale name that no
  // longer routes would only make the first message fail
  const saved = useOptionalPreferences()?.preferences?.default_playground_model ?? null;
  const preferredModel = saved && models.some((option) => option.id === saved) ? saved : preferred;
  // a column's thread lives in the column, so a column needs an identity that
  // outlasts its position: removing the first of two must not hand its thread
  // to the one that moved up
  const nextId = React.useRef(1);
  const [cols, setCols] = React.useState<{ id: number; model: string }[]>([{ id: 0, model: FAKE }]);
  const [multimodal, setMultimodal] = React.useState(false);
  // the list arrives after the first render, and can be replaced once — the
  // fallback first, then the gateway's own when a renewed or pasted key
  // works. until the operator picks something, the column follows the
  // catalog's choice: a deployment with models configured should not open on
  // lorem ipsum, and an earlier pick must not outlive the list it came from
  const touched = React.useRef(false);
  const auto = React.useRef(FAKE);
  React.useEffect(() => {
    if (touched.current || !preferredModel) return;
    const previous = auto.current;
    auto.current = preferredModel;
    setCols((c) =>
      c.length === 1 && c[0].model === previous ? [{ ...c[0], model: preferredModel }] : c,
    );
  }, [preferredModel]);
  const compare = cols.length > 1;
  const setModel = (i: number, v: string) => {
    touched.current = true;
    setCols((c) => c.map((col, j) => (j === i ? { ...col, model: v } : col)));
  };
  const add = () => {
    const id = nextId.current++;
    setCols((c) => [...c, { id, model: models[c.length % models.length]?.id ?? FAKE }]);
  };
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
            key={c.id}
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
              position={compare ? i + 1 : null}
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
                aria-label={t("pages.playground.textRowAria", { n: i + 1 })}
                className="h-8 text-sm"
              />
              <Button
                size="icon"
                variant="ghost"
                className="h-8 w-8"
                onClick={() => removeField(i)}
                aria-label={t("pages.playground.removeText", { n: i + 1 })}
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
          <GatewayButton size="sm" onClick={run} disabled={busy}>
            {busy ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}{" "}
            {t("pages.playground.embedProject")}
          </GatewayButton>
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
          aria-label={t("pages.playground.imagePromptAria")}
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
          <GatewayButton size="sm" onClick={gen} disabled={busy}>
            {busy ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <ImageIcon className="h-3.5 w-3.5" />
            )}{" "}
            {t("pages.playground.generate")}
          </GatewayButton>
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
function AudioMode({ models, active }: { models: ModelOption[]; active: boolean }) {
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
  const clipRef = React.useRef<HTMLAudioElement>(null);

  // the panel stays mounted under another tab, where a clip that kept playing
  // would have no control left to stop it
  React.useEffect(() => {
    if (!active) clipRef.current?.pause();
  }, [active]);

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
              aria-label={t("pages.playground.speechTextAria")}
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
              <GatewayButton size="sm" onClick={speak} disabled={busy}>
                {busy ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Mic className="h-3.5 w-3.5" />
                )}{" "}
                {t("pages.playground.synthesize")}
              </GatewayButton>
            </div>
            <ErrorNote error={error} />
          </div>
          <div className="rounded-lg border border-[color:var(--border-default)] bg-card p-4">
            <p className="mb-2.5 font-mono text-xs text-muted-foreground">
              {t("pages.playground.output")}
            </p>
            {audioUrl ? (
              <audio ref={clipRef} controls src={audioUrl} className="w-full" />
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
            <GatewayButton
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
            </GatewayButton>
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
          {/* stopping a live session sends nothing, so only starting one waits
              for a key */}
          <GatewayButton
            hold={!live}
            size="sm"
            variant={live ? "destructive" : "default"}
            onClick={live ? stop : start}
          >
            <Mic className="h-3.5 w-3.5" />{" "}
            {live ? t("pages.playground.stopSession") : t("pages.playground.startSession")}
          </GatewayButton>
        </span>
      </div>
      {/* the other tabs keep their work; this one cannot, and saying so here
          beats an operator finding an empty log after a glance elsewhere */}
      <p className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.playground.realtimeResets")}
      </p>
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
                onKeyDown={(e) => isSendKey(e) && send()}
                placeholder={t("pages.playground.framePlaceholder")}
                aria-label={t("pages.playground.frameAria")}
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
  const session = useKeySession();
  const catalog = useModelCatalog(session);
  const { options: models, source, ready, preferred, hidden } = catalog;
  const sendGate = useSendGate(session, catalog);

  // UX stream (#805); the screen key comes from the enclosing UxScreenProvider.
  // Playground is the screen an evaluator spends the most time in, so its
  // time-to-interactive is the number most worth having (#1730)
  useScreenReady(ready);

  return (
    <SendGateContext.Provider value={sendGate}>
      <PageBody>
        <SessionKeyBar session={session} rejected={catalog.rejected} keyless={catalog.keyless} />
        <ModelSourceNotice source={source} hidden={hidden} />
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
        {/* chat, embeddings, image and audio stay mounted under the other tabs,
            so a thread, a prompt, a result and a request still in flight all
            survive a switch: an image costs money to generate, and glancing at
            another tab must not throw it away. realtime is the exception, since
            a mounted one would hold its WebSocket open under a tab nobody is
            looking at: it is unmounted, which closes the socket, and starts
            over */}
        <div hidden={mode !== "chat"}>
          <ChatMode models={models} preferred={preferred} />
        </div>
        <div hidden={mode !== "embeddings"}>
          <EmbeddingsMode models={models} />
        </div>
        <div hidden={mode !== "image"}>
          <ImageMode models={models} />
        </div>
        <div hidden={mode !== "audio"}>
          <AudioMode models={models} active={mode === "audio"} />
        </div>
        {mode === "realtime" && <RealtimeMode models={models} />}
      </PageBody>
    </SendGateContext.Provider>
  );
}
