import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Ban, Bot, Loader2, Plus, RefreshCw } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { MAPPABLE_ROLES } from "@/components/GroupMappings";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import {
  ORG_TARGET,
  OrgScopePicker,
  OrgScopePill,
  scopeTargetIds,
  useOrgScope,
  type ScopeTarget,
} from "@/components/OrgScopePicker";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  RowIconButton,
  primaryColumn,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { SecretRevealDialog } from "@/components/ui/secret-reveal";
import {
  apiTokenState,
  fetchApiTokens,
  fetchCustomRoles,
  mintApiToken,
  revokeApiToken,
  rotateApiToken,
  type ApiTokenRow,
  type ApiTokenState,
  type MintedApiToken,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { roleLabel } from "@/lib/roles";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

const TOKENS_KEY = "api-tokens";

// the server's default when a mint names no expiry is 90 days
const DEFAULT_EXPIRY = "90";
const NEVER = "never";
const EXPIRY_DAYS = ["30", "90", "180", "365", "730"];

const GRID = `${primaryColumn(1.4)} 1.1fr 1fr 130px 130px 130px 100px 96px`;

const STATE_KEY: Record<ApiTokenState, string> = {
  active: "pages.apiTokens.state.active",
  expired: "pages.apiTokens.state.expired",
  revoked: "pages.apiTokens.state.revoked",
};

const STATE_TONE: Record<ApiTokenState, "success" | "neutral" | "danger"> = {
  active: "success",
  expired: "neutral",
  revoked: "danger",
};

/** The role a token holds: a built-in one, or the org's custom role by name. */
function useRoleName(orgId: string | undefined) {
  const { t } = useTranslation();
  // best-effort: a caller who may not read custom roles still sees the token,
  // with "Custom role" in place of the role's name
  const custom = useQuery({
    queryKey: ["custom-roles", orgId],
    queryFn: () => fetchCustomRoles(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  return {
    custom,
    nameOf: (token: ApiTokenRow): string => {
      if (token.role) return roleLabel(t, token.role);
      return (
        custom.data?.find((role) => role.id === token.custom_role_id)?.name ??
        t("pages.apiTokens.customRole")
      );
    },
  };
}

function MintSheet({
  open,
  onOpenChange,
  orgId,
  onMinted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  onMinted: (token: MintedApiToken) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const customRoles = useQuery({
    queryKey: ["custom-roles", orgId],
    queryFn: () => fetchCustomRoles(orgId),
    enabled: open,
    retry: false,
  });
  const [name, setName] = React.useState("");
  const [target, setTarget] = React.useState<ScopeTarget>(ORG_TARGET);
  // "role:<built-in>" or "custom:<id>"
  const [role, setRole] = React.useState("role:member");
  const [expiry, setExpiry] = React.useState(DEFAULT_EXPIRY);

  React.useEffect(() => {
    if (!open) return;
    setName("");
    setTarget(ORG_TARGET);
    setRole("role:member");
    setExpiry(DEFAULT_EXPIRY);
  }, [open]);

  const mint = useMutation({
    mutationFn: () => {
      const ids = scopeTargetIds(target);
      return mintApiToken(orgId, {
        name: name.trim(),
        ...ids,
        ...(role.startsWith("custom:")
          ? { custom_role_id: role.slice("custom:".length) }
          : { role: role.slice("role:".length) }),
        ...(expiry === NEVER ? { never_expires: true } : { expires_in_days: Number(expiry) }),
      });
    },
    onSuccess: (token) => {
      onMinted(token);
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name.trim() }),
        detail: errorText(error, t),
      });
    },
  });

  // the secret has been handed to the reveal; it must not linger in the
  // mutation's result once the sheet is gone
  const { isSuccess: minted, reset: resetMint } = mint;
  React.useEffect(() => {
    if (!open && minted) resetMint();
  }, [open, minted, resetMint]);

  const roleOptions: ComboboxOption[] = [
    ...MAPPABLE_ROLES.map((r) => ({
      value: `role:${r}`,
      label: roleLabel(t, r),
      group: t("pages.apiTokens.mint.builtInRoles"),
    })),
    ...(customRoles.data ?? []).map((r) => ({
      value: `custom:${r.id}`,
      label: r.name,
      description: r.slug,
      group: t("pages.apiTokens.mint.customRoles"),
    })),
  ];
  const expiryOptions: ComboboxOption[] = [
    ...EXPIRY_DAYS.map((days) => ({
      value: days,
      label: t("pages.apiTokens.mint.days", { count: Number(days) }),
    })),
    { value: NEVER, label: t("pages.apiTokens.mint.never") },
  ];

  return (
    <EditorSheet
      name="api-token-create"
      open={open}
      onOpenChange={onOpenChange}
      title={t("pages.apiTokens.mint.title")}
      subtitle={t("pages.apiTokens.mint.subtitle")}
      dirty={
        !!name.trim() ||
        target !== ORG_TARGET ||
        role !== "role:member" ||
        expiry !== DEFAULT_EXPIRY
      }
      errorMessage={mint.isError ? errorText(mint.error, t) : undefined}
      saveLabel={t("pages.apiTokens.mint.save")}
      canSave={!!name.trim() && name.trim().length <= 80}
      saving={mint.isPending}
      onSubmit={() => mint.mutate()}
    >
      <Field
        label={t("pages.apiTokens.mint.name")}
        hint={t("pages.apiTokens.mint.nameHint")}
        required
      >
        <Input
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("pages.apiTokens.mint.namePlaceholder")}
        />
      </Field>
      <Field label={t("pages.apiTokens.mint.scope")} hint={t("pages.apiTokens.mint.scopeHint")}>
        <OrgScopePicker
          orgId={orgId}
          value={target}
          onChange={setTarget}
          label={t("pages.apiTokens.mint.scope")}
          size="default"
          className="w-full"
        />
      </Field>
      <Field label={t("pages.apiTokens.mint.role")} hint={t("pages.apiTokens.mint.roleHint")}>
        <Combobox value={role} onChange={setRole} options={roleOptions} />
      </Field>
      <Field label={t("pages.apiTokens.mint.expiry")} hint={t("pages.apiTokens.mint.expiryHint")}>
        <Combobox value={expiry} onChange={setExpiry} options={expiryOptions} />
      </Field>
      {expiry === NEVER && (
        <p className="text-sm font-medium text-[color:var(--status-warning-text)]">
          {t("pages.apiTokens.mint.neverWarning")}
        </p>
      )}
    </EditorSheet>
  );
}

/**
 * Governance › API tokens (#3289).
 *
 * Scoped control-plane tokens (#1836) for CI jobs and provisioning scripts:
 * which automation holds what, when it expires and when it last ran. Minting
 * and rotating show the secret once; revoking takes it out of service at its
 * next request.
 */
export default function ApiTokens() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  const orgId = scope.orgId;
  const orgScope = useOrgScope(orgId);
  const { nameOf } = useRoleName(orgId);

  const tokens = useQuery({
    queryKey: [TOKENS_KEY, orgId],
    queryFn: () => fetchApiTokens(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  useScreenReady(!tokens.isLoading);
  const invalidate = () => queryClient.invalidateQueries({ queryKey: [TOKENS_KEY, orgId] });

  const [mintOpen, setMintOpen] = React.useState(false);
  const [minted, setMinted] = React.useState<{
    token: MintedApiToken;
    rotated: boolean;
  } | null>(null);
  const [rotateTarget, setRotateTarget] = React.useState<ApiTokenRow | null>(null);
  const [revokeTarget, setRevokeTarget] = React.useState<ApiTokenRow | null>(null);

  const rotate = useMutation({
    mutationFn: (token: ApiTokenRow) => rotateApiToken(token.id),
    onSuccess: (token) => {
      invalidate();
      setRotateTarget(null);
      setMinted({ token, rotated: true });
    },
  });
  const revoke = useMutation({
    mutationFn: (token: ApiTokenRow) => revokeApiToken(token.id),
    onSuccess: (_row, token) => {
      invalidate();
      setRevokeTarget(null);
      toast.push({
        tone: "success",
        title: t("pages.apiTokens.revokedToast", { name: token.name }),
      });
    },
  });

  if (scope.isLoading) {
    return (
      <PageBody>
        <ListSkeleton rows={4} />
      </PageBody>
    );
  }

  const rows = tokens.data ?? [];
  const canManage = !!orgId && !tokens.isError;
  const now = Date.now();

  return (
    <PageBody>
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="text-sm font-medium text-foreground">{t("pages.apiTokens.title")}</h2>
        <ListSummary data={tokens.data}>
          {(all) => t("pages.apiTokens.count", { count: all.length })}
        </ListSummary>
        <GatedButton
          gate="api_token:create"
          control="api-token-new"
          className="ml-auto"
          disabled={!canManage}
          onClick={() => setMintOpen(true)}
        >
          <Plus className="h-4 w-4" aria-hidden />
          {t("pages.apiTokens.mint.open")}
        </GatedButton>
      </div>

      {tokens.isError && (
        <LoadError
          error={tokens.error}
          resource={t("errors.resources.apiTokens")}
          onRetry={() => tokens.refetch()}
          target="api-tokens"
        />
      )}

      {!tokens.isError && (
        <ListTable label={t("screens.api-tokens.title")} minWidth={1040}>
          <ListHeader grid={GRID}>
            <ListHeaderCell>{t("pages.apiTokens.colName")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colScope")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colRole")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colPrefix")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colExpires")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colLastUsed")}</ListHeaderCell>
            <ListHeaderCell>{t("pages.apiTokens.colState")}</ListHeaderCell>
            <ListActionsHeader />
          </ListHeader>
          <ListLoadingRow read={tokens}>
            <ListSkeleton rows={3} className="p-3" />
          </ListLoadingRow>
          {rows.map((token) => {
            const state = apiTokenState(token, now);
            const live = state === "active";
            return (
              <ListRow key={token.id} grid={GRID}>
                <ListCell className="min-w-0 truncate text-sm font-medium">{token.name}</ListCell>
                <ListCell className="min-w-0">
                  <OrgScopePill scope={orgScope} value={token} />
                </ListCell>
                <ListCell>
                  <Badge tone="neutral">{nameOf(token)}</Badge>
                </ListCell>
                <ListCell className="truncate font-mono text-xs text-muted-foreground">
                  {token.token_prefix}…
                </ListCell>
                <ListCell className="font-mono text-xs text-muted-foreground">
                  {token.expires_at ? fmt.date(token.expires_at) : t("pages.apiTokens.noExpiry")}
                </ListCell>
                <ListCell className="font-mono text-xs text-muted-foreground">
                  {token.last_used_at
                    ? fmt.date(token.last_used_at)
                    : t("pages.apiTokens.neverUsed")}
                </ListCell>
                <ListCell>
                  <Badge dot={live} tone={STATE_TONE[state]}>
                    {t(STATE_KEY[state])}
                  </Badge>
                </ListCell>
                <ListCell className="flex justify-end gap-[5px]">
                  <RowIconButton
                    gate="api_token:update"
                    control="api-token-rotate"
                    title={
                      live
                        ? t("pages.apiTokens.rotate", { name: token.name })
                        : t("pages.apiTokens.inactiveHint")
                    }
                    aria-label={t("pages.apiTokens.rotate", { name: token.name })}
                    disabled={!live}
                    onClick={() => {
                      rotate.reset();
                      setRotateTarget(token);
                    }}
                  >
                    {rotate.isPending && rotate.variables?.id === token.id ? (
                      <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
                    ) : (
                      <RefreshCw className="h-3.5 w-3.5" />
                    )}
                  </RowIconButton>
                  <RowIconButton
                    className="hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)]"
                    gate="api_token:delete"
                    control="api-token-revoke"
                    title={
                      state === "revoked"
                        ? t("pages.apiTokens.alreadyRevoked")
                        : t("pages.apiTokens.revoke", { name: token.name })
                    }
                    aria-label={t("pages.apiTokens.revoke", { name: token.name })}
                    disabled={state === "revoked"}
                    onClick={() => {
                      revoke.reset();
                      setRevokeTarget(token);
                    }}
                  >
                    <Ban className="h-3.5 w-3.5" />
                  </RowIconButton>
                </ListCell>
              </ListRow>
            );
          })}
          <ListEmptyRow read={tokens} rows={rows.length}>
            <EmptyState
              thread
              uxTarget="api-tokens"
              className="py-6"
              icon={<Bot />}
              title={t("pages.apiTokens.empty.title")}
              description={t("pages.apiTokens.empty.body")}
              actions={
                <GatedButton
                  gate="api_token:create"
                  control="api-token-new-empty"
                  disabled={!canManage}
                  onClick={() => setMintOpen(true)}
                >
                  <Plus className="h-4 w-4" aria-hidden />
                  {t("pages.apiTokens.mint.open")}
                </GatedButton>
              }
            />
          </ListEmptyRow>
        </ListTable>
      )}

      {orgId && (
        <MintSheet
          open={mintOpen}
          onOpenChange={setMintOpen}
          orgId={orgId}
          onMinted={(token) => {
            invalidate();
            setMinted({ token, rotated: false });
          }}
        />
      )}

      <ConfirmDialog
        name="api-token-rotate"
        open={!!rotateTarget}
        onOpenChange={(open) => {
          if (open) return;
          setRotateTarget(null);
          rotate.reset();
        }}
        title={t("pages.apiTokens.confirm.rotateTitle", { name: rotateTarget?.name })}
        description={t("pages.apiTokens.confirm.rotateBody")}
        confirmLabel={t("pages.apiTokens.confirm.rotate")}
        pending={rotate.isPending}
        error={rotate.error}
        onConfirm={() => rotateTarget && rotate.mutate(rotateTarget)}
      />

      <ConfirmDialog
        name="api-token-revoke"
        open={!!revokeTarget}
        onOpenChange={(open) => {
          if (open) return;
          setRevokeTarget(null);
          revoke.reset();
        }}
        title={t("pages.apiTokens.confirm.revokeTitle", { name: revokeTarget?.name })}
        description={t("pages.apiTokens.confirm.revokeBody")}
        confirmLabel={t("pages.apiTokens.confirm.revoke")}
        pending={revoke.isPending}
        error={revoke.error}
        onConfirm={() => revokeTarget && revoke.mutate(revokeTarget)}
      />

      {/* the secret is shown once, after a mint or a rotation, and dropped on close */}
      <SecretRevealDialog
        name="api-token-revealed"
        open={!!minted}
        onOpenChange={(open) => {
          if (open) return;
          setMinted(null);
          // the rotated secret is dropped from the mutation's result with the reveal
          rotate.reset();
        }}
        title={t(
          minted?.rotated
            ? "pages.apiTokens.revealed.rotatedTitle"
            : "pages.apiTokens.revealed.title",
          { name: minted?.token.name },
        )}
        description={t(
          minted?.rotated
            ? "pages.apiTokens.revealed.rotatedBody"
            : "pages.apiTokens.revealed.body",
        )}
        secret={minted?.token.secret ?? ""}
        copyLabel={t("pages.apiTokens.revealed.copy")}
        size="lg"
      >
        <p className="text-sm text-muted-foreground">{t("pages.apiTokens.revealed.nextStep")}</p>
      </SecretRevealDialog>
    </PageBody>
  );
}
