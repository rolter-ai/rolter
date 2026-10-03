import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleDollarSign, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { CardStack } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import { ListSummary, PageBody, Toolbar } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { EmptyState } from "@/components/ui/empty-state";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  deleteModelPrice,
  fetchAnalyticsByModel,
  fetchCurrencySettings,
  fetchModels,
  fetchModelPrices,
  isConvertible,
  upsertModelPrice,
  type CurrencySettings,
  type ModelPriceRow,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PRICES_QUERY_KEY = ["model-prices"];

// global model pricing catalog — no org/team/project scoping. upsert is
// keyed on `model`, so add and edit share one dialog/mutation.
export default function Pricing() {
  const queryClient = useQueryClient();
  const toast = useToast();

  const prices = useQuery({
    queryKey: PRICES_QUERY_KEY,
    queryFn: fetchModelPrices,
  });
  // deployment config, not screen data: a stored price in a code the rate
  // table does not carry is unconvertible, and reads as zero spend rather
  // than as an error (#965)
  const currency = useQuery({
    queryKey: ["currency-settings"],
    queryFn: fetchCurrencySettings,
    staleTime: Infinity,
    retry: false,
  });
  const { t } = useTranslation();

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `prices` is the query the user is actually waiting on for this screen

  useScreenReady(!prices.isLoading);

  useErrorState(!!prices.error, "pricing");

  const invalidate = () => queryClient.invalidateQueries({ queryKey: PRICES_QUERY_KEY });

  const removePrice = useMutation({
    mutationFn: (model: string) => deleteModelPrice(model),
    onSuccess: invalidate,
  });

  const [editOpen, setEditOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<ModelPriceRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<ModelPriceRow | null>(null);
  // model prices are deployment-wide, so a row control is the superadmin's
  // exactly as the add button is (#1258)

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={prices.data}>
          {(rows) => t("pages.pricing.summary", { count: rows.length })}
        </ListSummary>
        {/* a price is written with PUT /model-prices whether or not the row
            exists, so adding one takes `model_price:update` — there is no
            create capability to gate on (#1258) */}
        <GatedButton
          gate="model_price:update"
          control="price-new"
          className="ml-auto"
          onClick={() => {
            setEditTarget(null);
            setEditOpen(true);
          }}
        >
          <Plus className="h-4 w-4" />
          {t("pages.pricing.emptyAction")}
        </GatedButton>
      </Toolbar>

      {prices.isLoading && <CardGridSkeleton cards={4} height={152} min={300} />}
      {prices.error && (
        <LoadError
          error={prices.error}
          resource={t("errors.resources.modelPrices")}
          onRetry={() => prices.refetch()}
        />
      )}
      {!prices.isLoading && prices.data?.length === 0 && (
        <EmptyState
          uxTarget="model-prices"
          icon={<CircleDollarSign />}
          title={t("pages.pricing.emptyTitle")}
          description={t("pages.pricing.emptyBody")}
          actions={
            <GatedButton
              gate="model_price:update"
              control="price-new-empty"
              onClick={() => {
                setEditTarget(null);
                setEditOpen(true);
              }}
            >
              {t("pages.pricing.emptyAction")}
            </GatedButton>
          }
        />
      )}

      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(300px,100%),1fr))]">
        {prices.data?.map((price) => (
          <CardStack key={price.id}>
            <div className="truncate font-mono text-sm font-semibold">{price.model}</div>
            <div className="flex flex-wrap gap-1.5">
              <Badge tone="outline">
                {t("pages.pricing.inPrice", {
                  value: price.input_per_mtok,
                  currency: price.currency,
                })}
              </Badge>
              <Badge tone="outline">
                {t("pages.pricing.outPrice", {
                  value: price.output_per_mtok,
                  currency: price.currency,
                })}
              </Badge>
              {price.cached_input_per_mtok && (
                <Badge tone="neutral">
                  {t("pages.pricing.cachedPrice", {
                    value: price.cached_input_per_mtok,
                    currency: price.currency,
                  })}
                </Badge>
              )}
            </div>
            {!isConvertible(currency.data, price.currency) && (
              <p className="text-xs text-[color:var(--status-warning-text)]">
                {t("pages.pricing.unconvertible", {
                  code: price.currency,
                  base: currency.data?.base ?? "",
                })}
              </p>
            )}
            <div className="flex justify-end gap-2 border-t border-[color:var(--border-subtle)] pt-2.5">
              <GatedButton
                gate="model_price:update"
                control="price-edit"
                size="sm"
                variant="outline"
                aria-label={t("pages.pricing.editAria", { model: price.model })}
                onClick={() => {
                  setEditTarget(price);
                  setEditOpen(true);
                }}
              >
                {t("pages.pricing.edit")}
              </GatedButton>
              <DeleteIconButton
                gate="model_price:delete"
                control="price-delete"
                label={t("pages.pricing.deleteAria", { model: price.model })}
                pending={removePrice.isPending && deleteTarget?.model === price.model}
                onClick={() => setDeleteTarget(price)}
              />
            </div>
          </CardStack>
        ))}
      </div>

      <UpsertPriceDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        existing={editTarget}
        currency={currency.data}
        onDone={invalidate}
      />

      <ConfirmDialog
        name="price-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDeleteTarget(null);
          // a refusal for this price must not greet the next one opened
          removePrice.reset();
        }}
        title={t("pages.pricing.confirm.deleteTitle", { model: deleteTarget?.model ?? "" })}
        description={t("pages.pricing.confirm.deleteBody")}
        confirmLabel={t("pages.pricing.confirm.deleteConfirm")}
        pending={removePrice.isPending}
        error={removePrice.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.model;
          removePrice.mutate(what, {
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
    </PageBody>
  );
}

function UpsertPriceDialog({
  open,
  onOpenChange,
  existing,
  currency: settings,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing: ModelPriceRow | null;
  currency: CurrencySettings | undefined;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [model, setModel] = React.useState("");
  // empty is "not set yet", never 0: a zero price reads as free, and a model
  // saved at zero drops out of the unpriced flags (#2100)
  const [inputPerMtok, setInputPerMtok] = React.useState("");
  const [outputPerMtok, setOutputPerMtok] = React.useState("");
  const [cachedInputPerMtok, setCachedInputPerMtok] = React.useState("");
  const baseCurrency = settings?.base ?? "USD";
  const [currency, setCurrency] = React.useState(baseCurrency);
  const [attempted, setAttempted] = React.useState(false);

  // names worth offering: the routes the gateway serves and the models seen in
  // traffic. both are suggestions — an analytics store may be absent
  const routeModels = useQuery({ queryKey: ["models"], queryFn: fetchModels, enabled: open });
  const trafficModels = useQuery({
    queryKey: ["analytics", "by-model", "price-picker"],
    queryFn: () => fetchAnalyticsByModel(),
    enabled: open,
    retry: false,
  });
  const modelOptions = React.useMemo<ComboboxOption[]>(() => {
    const routes = (routeModels.data ?? []).map((r) => r.model);
    const seen = new Set(routes);
    const traffic = (trafficModels.data ?? []).map((r) => r.model).filter((m) => m && !seen.has(m));
    return [
      ...routes.map((value) => ({
        value,
        label: value,
        group: t("pages.pricing.modelGroupRoutes"),
      })),
      ...[...new Set(traffic)].map((value) => ({
        value,
        label: value,
        group: t("pages.pricing.modelGroupTraffic"),
      })),
    ];
  }, [routeModels.data, trafficModels.data, t]);
  const currencyOptions = React.useMemo<ComboboxOption[]>(() => {
    const codes = [baseCurrency, ...(settings?.codes ?? [])];
    return [...new Set(codes)].map((code) => ({
      value: code,
      label: code,
      description: code === baseCurrency ? t("pages.pricing.currencyBase") : undefined,
    }));
  }, [baseCurrency, settings, t]);

  React.useEffect(() => {
    if (open) {
      setAttempted(false);
      setModel(existing?.model ?? "");
      setInputPerMtok(existing?.input_per_mtok ?? "");
      setOutputPerMtok(existing?.output_per_mtok ?? "");
      setCachedInputPerMtok(existing?.cached_input_per_mtok ?? "");
      setCurrency(existing?.currency ?? baseCurrency);
    }
  }, [open, existing]);

  const submit = useMutation({
    mutationFn: () =>
      upsertModelPrice({
        model,
        input_per_mtok: inputPerMtok,
        output_per_mtok: outputPerMtok,
        cached_input_per_mtok: cachedInputPerMtok.trim() || undefined,
        currency,
      }),
    onSuccess: () => {
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push(
        existing
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: model }),
            }
          : { tone: "success", title: t("toast.created", { what: model }) },
      );
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: model }),
        detail: errorDetail(error),
      });
    },
  });

  // a price is valid when it was typed and is a number of 0 or more
  const priceError = (value: string) =>
    !value.trim()
      ? t("pages.pricing.priceRequired")
      : !(Number(value) >= 0)
        ? t("pages.pricing.priceInvalid")
        : undefined;
  const errors = {
    model: model.trim() ? undefined : t("pages.pricing.modelRequired"),
    input: priceError(inputPerMtok),
    output: priceError(outputPerMtok),
    // an optional price is only wrong when it was typed and is not a number
    cached: cachedInputPerMtok.trim() ? priceError(cachedInputPerMtok) : undefined,
    currency: currency.trim() ? undefined : t("pages.pricing.currencyRequired"),
  };
  const invalid = Object.values(errors).some(Boolean);
  const shown: Partial<typeof errors> = attempted ? errors : {};

  const dirty =
    model.trim() !== (existing?.model ?? "") ||
    inputPerMtok !== (existing?.input_per_mtok ?? "") ||
    outputPerMtok !== (existing?.output_per_mtok ?? "") ||
    cachedInputPerMtok !== (existing?.cached_input_per_mtok ?? "") ||
    currency !== (existing?.currency ?? baseCurrency);

  return (
    <EditorSheet
      name={existing ? "model-price-edit" : "model-price-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={
        existing
          ? t("pages.pricing.editTitle", { model: existing.model })
          : t("pages.pricing.emptyAction")
      }
      subtitle={t("pages.pricing.editSubtitle")}
      dirty={dirty}
      errorMessage={submit.isError ? (submit.error as Error).message : undefined}
      saveLabel={t("common.save")}
      canSave
      saving={submit.isPending}
      onSave={() => {
        // the refusal is shown at the fields, not by greying the button out
        setAttempted(true);
        if (!invalid) submit.mutate();
      }}
    >
      <div className="space-y-3">
        <Field label={t("pages.pricing.modelName")} error={shown.model}>
          <Combobox
            allowCustom
            options={modelOptions}
            value={model}
            onChange={setModel}
            placeholder={t("pages.pricing.modelPlaceholder")}
            disabled={!!existing}
          />
        </Field>
        <Field
          label={t("pages.pricing.inputPrice")}
          hint={t("pages.pricing.priceUnit", { currency })}
          error={shown.input}
        >
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={inputPerMtok}
            onChange={(e) => setInputPerMtok(e.target.value)}
          />
        </Field>
        <Field
          label={t("pages.pricing.outputPrice")}
          hint={t("pages.pricing.priceUnit", { currency })}
          error={shown.output}
        >
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={outputPerMtok}
            onChange={(e) => setOutputPerMtok(e.target.value)}
          />
        </Field>
        <Field
          label={t("pages.pricing.cachedInputPrice")}
          hint={t("pages.pricing.priceUnit", { currency })}
          error={shown.cached}
        >
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={cachedInputPerMtok}
            onChange={(e) => setCachedInputPerMtok(e.target.value)}
            placeholder={t("pages.pricing.cachedPlaceholder")}
          />
        </Field>
        <Field
          label={t("pages.pricing.currency")}
          hint={
            isConvertible(settings, currency)
              ? t("pages.pricing.currencyHint")
              : t("pages.pricing.unconvertible", { code: currency, base: baseCurrency })
          }
          info={t("pages.pricing.currencyInfo")}
          error={shown.currency}
        >
          <Combobox allowCustom options={currencyOptions} value={currency} onChange={setCurrency} />
        </Field>
      </div>
    </EditorSheet>
  );
}
