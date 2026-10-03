import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Plus, RotateCw } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DocsLink } from "@/components/DocsLink";
import {
  DEFAULT_KEY_TTL_DAYS,
  KeyCacheField,
  KeyExpiryField,
  KeyModelsField,
  KeyNameField,
  KeyReachSummary,
  keyNameProblem,
  parseCacheMode,
  useRouteModels,
  ttlToDays,
  type CacheMode,
} from "@/components/KeyMintFields";
import { KeyNextStep } from "@/components/KeyNextStep";
import { KeyProvidersField } from "@/components/KeyAttributionFields";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EmptyState } from "@/components/ui/empty-state";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { ListSummary, PageBody, Toolbar } from "@/components/screen";
import { SelfServiceUnavailable } from "@/components/SelfServiceUnavailable";
import { ProfileCard } from "@/components/ProfileCard";
import { TwoFactorPanel } from "@/components/TwoFactorPanel";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { SecretRevealDialog } from "@/components/ui/secret-reveal";
import { Tag } from "@/components/ui/tag";
import {
  AnalyticsUnavailableError,
  PLAYGROUND_PURPOSE,
  deleteMyKey,
  fetchMyKeys,
  fetchProviders,
  isOpenModeNoSession,
  fetchMyUsage,
  mintMyKey,
  rotateMyKey,
  type MintedKey,
  type MyUsageRow,
  type OwnedKeyRow,
  type ProviderRow,
} from "@/lib/api";
import { useCan } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { describeError } from "@/lib/error-copy";
import { errorDetail, useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

// end-user self-service panel (ROL-224): view/rotate/delete the virtual keys you
// personally minted and see your own usage/spend. no admin role required — the
// backend scopes everything to the logged-in account.
export default function Account() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();

  const keys = useQuery({ queryKey: ["my-keys"], queryFn: fetchMyKeys });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `keys` is the query the user is actually waiting on for this screen

  useScreenReady(!keys.isLoading);

  const usage = useQuery({
    queryKey: ["my-usage"],
    queryFn: () => fetchMyUsage(),
    retry: false,
  });

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["my-keys"] });
    queryClient.invalidateQueries({ queryKey: ["my-usage"] });
  };

  const removeKey = useMutation({
    mutationFn: (id: string) => deleteMyKey(id),
    onSuccess: invalidate,
  });

  const [mintOpen, setMintOpen] = React.useState(false);
  const [minted, setMinted] = React.useState<MintedKey | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<OwnedKeyRow | null>(null);

  // usage rows keyed by virtual_key_id, for merging into each key card
  const usageByKey = React.useMemo(() => {
    const map = new Map<string, MyUsageRow>();
    for (const row of usage.data ?? []) map.set(row.virtual_key_id, row);
    return map;
  }, [usage.data]);

  // any usage failure, analytics-less deployment included, blanks every card's
  // figure: "no usage" beside a failed query would read as a key that spent
  // nothing. the reason is said once above the grid rather than per card (#1270)
  const usageUnavailable = !!usage.error;
  const selfServiceUnavailable = isOpenModeNoSession(keys.error);
  // minting is `my_virtual_key:create`, the member role at the project, the
  // same pair the Playground's mint asks (#2061). the buttons gate themselves;
  // this is for the empty state's copy, which only an explicit "no" changes
  const mintRefused = useCan()("my_virtual_key", "create") === false;

  // the provider allow-list needs the org's providers, which a plain member may
  // not be allowed to read. `retry: false` and an empty list on failure, so the
  // mint sheet drops the control rather than blocking on a 403 it cannot fix
  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });

  return (
    <PageBody>
      {/* who you are comes before how you sign in (#2434) */}
      <ProfileCard />
      {/* the second factor comes first: it protects the session that reaches
          every key below it, and an org policy can make it mandatory (#1078) */}
      <TwoFactorPanel />
      {/* the screen sits under "Account" in a product that also has provider
          keys and an admin token; say which credential this one is (#943) */}
      <p className="text-sm text-muted-foreground">
        {t("account.keys.explainer")}{" "}
        {/* the explainer stands alone; the link only adds depth, and is absent
            on a deployment that configured no documentation host (#1164) */}
        <DocsLink page="whichKey" label={t("docs.link.whichKey")} />
      </p>
      {/* a toolbar, not a bare flex row: the Russian button is ~250px wide, so
          at 375px it drops under the count instead of pushing the page wide (#2352) */}
      <Toolbar>
        <ListSummary data={keys.data}>
          {(rows) => t("account.keys.summary", { count: rows.length })}
        </ListSummary>
        <GatedButton
          gate="my_virtual_key:create"
          control="account-key-mint"
          className="ml-auto"
          onClick={() => setMintOpen(true)}
          // minting posts to /me/*, which 401s for the same reason the list
          // did; offering the button would just move the dead end one click
          // later (#942)
          disabled={!scope.projectId || selfServiceUnavailable}
          title={scope.projectId ? undefined : t("account.keys.selectProject")}
        >
          <Plus className="h-4 w-4" />
          {t("account.keys.generate")}
        </GatedButton>
      </Toolbar>

      {/* the content below is a card grid, so the placeholder holding its
          space is one too — and it is a `role="status"` region rather than a
          grey sentence no screen reader is told about (#1589) */}
      {keys.isLoading && (
        <CardGridSkeleton cards={3} height={168} min={320} testId="own-keys-loading" />
      )}
      {/* open mode is not a failed request, it is a screen this deployment
          cannot serve at all — saying so beats a red line about loading (#942) */}
      {selfServiceUnavailable && <SelfServiceUnavailable />}
      {keys.error && !selfServiceUnavailable && (
        <LoadError
          error={keys.error}
          resource={t("errors.resources.yourKeys")}
          onRetry={() => keys.refetch()}
          target="own-keys"
        />
      )}
      {!keys.isLoading && !keys.error && keys.data?.length === 0 && (
        <EmptyState
          data-testid="own-keys-empty"
          uxTarget="own-keys"
          icon={<KeyRound />}
          title={t("account.keys.emptyTitle")}
          // without a project the mint dialog has nowhere to post, so the
          // placeholder says what to do instead of offering a dead button. a
          // role that cannot mint is told who can and whom to ask, rather than
          // invited to make a key the button beside it refuses (#2064)
          description={
            !scope.projectId
              ? t("account.keys.selectProject")
              : mintRefused
                ? t("account.keys.emptyRefused")
                : t("account.keys.empty")
          }
          actions={
            scope.projectId && !selfServiceUnavailable ? (
              <GatedButton
                gate="my_virtual_key:create"
                control="account-key-mint-empty"
                onClick={() => setMintOpen(true)}
              >
                {t("account.keys.generate")}
              </GatedButton>
            ) : undefined
          }
        />
      )}

      {/* a deployment with no analytics store is a supported shape, not a
          failed read: it is said as a status with no retry, and the keys below
          stay as usable as they were (#2016) */}
      {usage.error &&
        !!keys.data?.length &&
        (usage.error instanceof AnalyticsUnavailableError ? (
          <AnalyticsUnavailable error={usage.error} i18nKey="account.keys.noAnalytics" />
        ) : (
          <LoadError
            error={usage.error}
            resource={t("errors.resources.yourUsage")}
            onRetry={() => usage.refetch()}
            target="own-usage"
          />
        ))}

      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(320px,100%),1fr))]">
        {keys.data?.map((key) => (
          <KeyCard
            key={key.id}
            keyRow={key}
            usage={usageByKey.get(key.id)}
            usageUnavailable={usageUnavailable}
            onRotated={(m) => {
              invalidate();
              setMinted(m);
            }}
            onDelete={() => setDeleteTarget(key)}
          />
        ))}
      </div>

      {scope.projectId && (
        <MintKeyDialog
          open={mintOpen}
          onOpenChange={setMintOpen}
          projectId={scope.projectId}
          projectLabel={scope.projects.find((p) => p.id === scope.projectId)?.name}
          providers={providers.data ?? []}
          onMinted={(m) => {
            invalidate();
            setMinted(m);
          }}
        />
      )}

      <ConfirmDialog
        name="account-key-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDeleteTarget(null);
          // a refusal for this key must not greet the next one opened
          removeKey.reset();
        }}
        // an unnamed key is still named by its prefix, the same fallback the
        // toast uses
        title={t("account.keys.deleteConfirm.title", {
          name: deleteTarget?.name ?? deleteTarget?.key_prefix ?? "",
        })}
        description={
          <Trans
            i18nKey="account.keys.deleteConfirm.body"
            values={{ prefix: `${deleteTarget?.key_prefix ?? ""}…` }}
            components={[<span key="prefix" className="font-mono" />]}
          />
        }
        confirmLabel={t("account.keys.deleteConfirm.confirm")}
        pending={removeKey.isPending}
        error={removeKey.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name ?? deleteTarget.key_prefix;
          removeKey.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) =>
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorDetail(error),
              }),
          });
        }}
      />

      {/* the plaintext is shown once, after a mint or a rotation, and dropped
          on close */}
      <SecretRevealDialog
        name="account-key-revealed"
        open={!!minted}
        onOpenChange={(open) => !open && setMinted(null)}
        title={t("account.keys.revealed.title")}
        description={t("account.keys.revealed.body")}
        secret={minted?.key ?? ""}
        copyLabel={t("common.copy")}
        doneLabel={t("account.keys.revealed.done")}
        size="lg"
      >
        <KeyNextStep models={minted?.models ?? []} />
      </SecretRevealDialog>
    </PageBody>
  );
}

function KeyCard({
  keyRow,
  usage,
  usageUnavailable,
  onRotated,
  onDelete,
}: {
  keyRow: OwnedKeyRow;
  usage?: { requests: number | string; cost_usd: number | string };
  usageUnavailable: boolean;
  onRotated: (m: MintedKey) => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const format = useFormat();
  const rotate = useMutation({
    mutationFn: () => rotateMyKey(keyRow.id),
    onSuccess: onRotated,
  });
  // rotation is not undoable and the old secret dies the instant the new one is
  // issued, so it asks first like every other destructive action (#1179)
  const [rotateOpen, setRotateOpen] = React.useState(false);
  const keyLabel = keyRow.name ?? t("account.keys.card.unnamed");
  // the name a control carries for this card: two unnamed keys would both be
  // "unnamed key", so the prefix tells them apart, as in the delete dialog (#1896)
  const keyRef = keyRow.name ?? keyRow.key_prefix;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between gap-2">
          <span className="truncate">{keyLabel}</span>
          {/* see Keys.tsx: a Playground-minted key says so, because it is the
              one kind of key here nobody created on purpose (#944) */}
          {keyRow.purpose === PLAYGROUND_PURPOSE && (
            <Badge tone="info" title={t("account.keys.card.playgroundHint")}>
              {t("account.keys.card.playground")}
            </Badge>
          )}
          <Badge tone={keyRow.disabled ? "danger" : "success"}>
            {keyRow.disabled ? t("account.keys.card.disabled") : t("account.keys.card.active")}
          </Badge>
        </CardTitle>
        <CardDescription className="font-mono">{keyRow.key_prefix}…</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-xs text-muted-foreground">
          {keyRow.org_name} / {keyRow.project_name}
        </p>
        <div className="flex flex-wrap gap-1.5">
          {keyRow.models.length ? (
            keyRow.models.map((m) => <Tag key={m}>{m}</Tag>)
          ) : (
            <Badge tone="neutral">{t("account.keys.card.allModels")}</Badge>
          )}
        </div>
        <div className="text-xs text-muted-foreground">
          {usageUnavailable ? (
            <span>{t("account.keys.card.usageUnavailable")}</span>
          ) : usage ? (
            <span>
              {t("account.keys.card.usage", {
                // `count` picks the plural form, `requests` is the figure as
                // the locale formats it
                count: Number(usage.requests),
                requests: format.number(Number(usage.requests)),
                cost: format.currency(Number(usage.cost_usd)),
              })}
            </span>
          ) : (
            <span>{t("account.keys.card.noUsage")}</span>
          )}
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {/* both controls name their card: N identical "Rotate" and "Delete"
              buttons are a list a screen reader cannot tell apart (#1214, #1896) */}
          <Button
            size="sm"
            variant="outline"
            className="h-[30px]"
            disabled={rotate.isPending}
            onClick={() => {
              rotate.reset();
              setRotateOpen(true);
            }}
            aria-label={t("account.keys.card.rotateAria", { name: keyRef })}
            title={t("account.keys.card.rotateHint")}
          >
            <RotateCw className="h-3.5 w-3.5" />
            {t("account.keys.card.rotate")}
          </Button>
          <DeleteIconButton
            label={t("account.keys.card.deleteAria", { name: keyRef })}
            onClick={onDelete}
          />
        </div>
        <ConfirmDialog
          name="account-key-rotate"
          open={rotateOpen}
          onOpenChange={setRotateOpen}
          title={t("account.keys.rotateConfirm.title", { name: keyLabel })}
          description={t("account.keys.rotateConfirm.body")}
          confirmLabel={t("account.keys.rotateConfirm.confirm")}
          pending={rotate.isPending}
          error={rotate.error}
          onConfirm={() => rotate.mutate(undefined, { onSuccess: () => setRotateOpen(false) })}
        />
      </CardContent>
    </Card>
  );
}

function MintKeyDialog({
  open,
  onOpenChange,
  projectId,
  projectLabel,
  providers,
  onMinted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  projectLabel?: string;
  providers: ProviderRow[];
  onMinted: (m: MintedKey) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = React.useState("");
  const [models, setModels] = React.useState<string[]>([]);
  const [ttl, setTtl] = React.useState(String(DEFAULT_KEY_TTL_DAYS));
  const [providerSel, setProviderSel] = React.useState<string[]>([]);
  const [cache, setCache] = React.useState<CacheMode>("inherit");
  // the models this project routes; only asked for while the sheet is open
  const routes = useRouteModels(projectId, open);

  React.useEffect(() => {
    if (open) {
      setName("");
      setModels([]);
      setTtl(String(DEFAULT_KEY_TTL_DAYS));
      setProviderSel([]);
      setCache("inherit");
    }
  }, [open]);

  const project = projectLabel ?? t("account.keys.mint.currentProject");

  const mint = useMutation({
    mutationFn: () =>
      mintMyKey(projectId, {
        name: name.trim(),
        models,
        providers: providerSel,
        cache: parseCacheMode(cache),
        expires_in_days: ttlToDays(ttl),
      }),
    onSuccess: (m) => {
      onOpenChange(false);
      onMinted(m);
    },
  });

  return (
    <EditorSheet
      name="account-key-mint"
      open={open}
      onOpenChange={onOpenChange}
      title={t("account.keys.mint.title")}
      subtitle={t("account.keys.mint.subtitle", { project })}
      dirty={Boolean(name || models.length || providerSel.length) || cache !== "inherit"}
      // the lead is ours and translated; the control plane's own words follow as
      // the detail, since the server answers in English whatever the locale
      errorMessage={mint.isError ? t("account.keys.mint.failed") : undefined}
      errorDetail={
        mint.isError
          ? (describeError(mint.error, t).detail ?? describeError(mint.error, t).message)
          : undefined
      }
      saveLabel={t("account.keys.mint.save")}
      canSave={keyNameProblem(name) === null}
      saving={mint.isPending}
      onSave={() => mint.mutate()}
    >
      <div className="space-y-3">
        <KeyNameField value={name} onChange={setName} />
        <KeyExpiryField value={ttl} onChange={setTtl} />
        <KeyCacheField value={cache} onChange={setCache} />
        <KeyModelsField
          value={models}
          onChange={setModels}
          options={routes.models}
          loading={routes.loading}
          error={routes.error}
          onRetry={routes.retry}
        />
        <KeyProvidersField providers={providers} selected={providerSel} onChange={setProviderSel} />
        <KeyReachSummary project={project} models={models} providers={providerSel} ttl={ttl} />
      </div>
    </EditorSheet>
  );
}
