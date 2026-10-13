import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { CircleDollarSign, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { SampledLogNotice } from "@/components/SampledLogNotice";
import { EditorSheet } from "@/components/EditorSheet";
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
  SearchInput,
  SortLabel,
  STICKY_ACTIONS,
  Toolbar,
  primaryColumn,
  useSort,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  AnalyticsUnavailableError,
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
import { cacheWrite1hPatch, cacheWritePatch } from "@/lib/cache-write-rate";
import { useErrorVisibility } from "@/lib/error-visibility";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";
import { useFormat } from "@/lib/i18n/format";
import { windowBounds } from "@/lib/time-window";
import { useFieldErrors } from "@/lib/use-field-errors";
import { useScreenReady } from "@/lib/ux-react";

const PRICES_QUERY_KEY = ["model-prices"];

/**
 * The rates a price row can carry, one table column each. A new kind of price
 * (a per-image or per-second rate) is one more entry here plus its catalog keys:
 * the header, the sort and the cell are all read from this list.
 */
const PRICE_COLUMNS = [
  { col: "input", field: "input_per_mtok" },
  { col: "output", field: "output_per_mtok" },
  { col: "cached", field: "cached_input_per_mtok" },
  { col: "cacheWrite", field: "cache_write_per_mtok" },
  { col: "cacheWrite1h", field: "cache_write_1h_per_mtok" },
] as const satisfies readonly { col: string; field: keyof ModelPriceRow }[];

type PriceColumn = (typeof PRICE_COLUMNS)[number]["col"];
type PriceSortColumn = "model" | "currency" | PriceColumn;

const GRID = `${primaryColumn(1.5)} repeat(${PRICE_COLUMNS.length}, minmax(5.5rem, 1fr)) 5rem 168px`;
const UNPRICED_GRID = `${primaryColumn(1.5)} minmax(7rem, 0.6fr) 168px`;

// the window the unpriced list reads: long enough to catch a model that is
// called a few times a week, short enough that a model priced since is gone
const UNPRICED_WINDOW = "7d";

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

  const invalidate = () => queryClient.invalidateQueries({ queryKey: PRICES_QUERY_KEY });

  const removePrice = useMutation({
    mutationFn: (model: string) => deleteModelPrice(model),
    onSuccess: invalidate,
  });

  const [editOpen, setEditOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<ModelPriceRow | null>(null);
  // the model an "Add price" from the unpriced list opens the sheet with
  const [seedModel, setSeedModel] = React.useState("");
  const [search, setSearch] = React.useState("");
  const { sort, cycle, apply } = useSort<PriceSortColumn>();
  const fmt = useFormat();
  const [deleteTarget, setDeleteTarget] = React.useState<ModelPriceRow | null>(null);
  // model prices are deployment-wide, so a row control is the superadmin's
  // exactly as the add button is (#1258)

  const q = search.trim().toLowerCase();
  const matched = (prices.data ?? []).filter((p) => !q || p.model.toLowerCase().includes(q));
  const rows = apply<ModelPriceRow>(matched, {
    model: (p) => p.model,
    currency: (p) => p.currency,
    ...(Object.fromEntries(
      // a rate that is not set sorts below every rate that is
      PRICE_COLUMNS.map(({ col, field }) => [col, (p: ModelPriceRow) => Number(p[field] ?? -1)]),
    ) as Record<PriceColumn, (p: ModelPriceRow) => number>),
  });
  // a rate as the dashboard's locale writes a number; `—` where none is set
  const priceText = (value: string | null | undefined) => {
    if (value == null || value === "") return "—";
    const n = Number(value);
    return Number.isFinite(n)
      ? fmt.number(n, { minimumFractionDigits: 2, maximumFractionDigits: 6 })
      : value;
  };

  return (
    <PageBody>
      <Toolbar>
        <SearchInput
          placeholder={t("pages.pricing.searchPlaceholder")}
          aria-label={t("pages.pricing.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
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
            setSeedModel("");
            setEditOpen(true);
          }}
        >
          <Plus className="h-4 w-4" />
          {t("pages.pricing.emptyAction")}
        </GatedButton>
      </Toolbar>

      <UnpricedModels
        priced={prices}
        onAdd={(model) => {
          setEditTarget(null);
          setSeedModel(model);
          setEditOpen(true);
        }}
      />

      {prices.error && (
        <LoadError
          error={prices.error}
          resource={t("errors.resources.modelPrices")}
          onRetry={() => prices.refetch()}
          target="model-prices"
        />
      )}

      <ListTable label={t("screens.pricing-overrides.title")} minWidth={1180}>
        <ListHeader grid={GRID}>
          <SortLabel
            label={t("pages.pricing.columns.model")}
            col="model"
            sort={sort}
            onCycle={(c) => cycle(c as PriceSortColumn)}
          />
          {PRICE_COLUMNS.map(({ col }) => (
            <SortLabel
              key={col}
              label={t(`pages.pricing.columns.${col}`)}
              col={col}
              sort={sort}
              onCycle={(c) => cycle(c as PriceSortColumn)}
              justify="flex-end"
            />
          ))}
          <SortLabel
            label={t("pages.pricing.columns.currency")}
            col="currency"
            sort={sort}
            onCycle={(c) => cycle(c as PriceSortColumn)}
          />
          <ListActionsHeader className={STICKY_ACTIONS.header} />
        </ListHeader>
        <ListLoadingRow read={prices}>
          <ListSkeleton rows={5} className="p-3" />
        </ListLoadingRow>
        {rows.map((price) => (
          <ListRow key={price.id} grid={GRID}>
            <ListCell className="flex min-w-0 flex-col gap-1">
              <span className="truncate font-mono text-sm font-semibold">{price.model}</span>
              {!isConvertible(currency.data, price.currency) && (
                <span className="text-xs text-[color:var(--status-warning-text)]">
                  {t("pages.pricing.unconvertible", {
                    code: price.currency,
                    base: currency.data?.base ?? "",
                  })}
                </span>
              )}
            </ListCell>
            {PRICE_COLUMNS.map(({ col, field }) => (
              <ListCell key={col} className="text-right font-mono text-sm tabular-nums">
                {priceText(price[field])}
              </ListCell>
            ))}
            <ListCell className="font-mono text-xs text-[color:var(--text-secondary)]">
              {price.currency}
            </ListCell>
            <ListCell className={`flex items-center justify-end gap-2 ${STICKY_ACTIONS.cell}`}>
              <GatedButton
                gate="model_price:update"
                control="price-edit"
                size="sm"
                variant="outline"
                aria-label={t("pages.pricing.editAria", { model: price.model })}
                onClick={() => {
                  setSeedModel("");
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
            </ListCell>
          </ListRow>
        ))}
        {/* "no prices" and "nothing matched the search" are different answers:
            one wants a price added, the other wants the search cleared */}
        <ListEmptyRow read={prices} rows={rows.length}>
          {q ? (
            <EmptyState
              uxTarget="model-prices"
              icon={<CircleDollarSign />}
              title={t("pages.pricing.noMatchTitle")}
              description={t("pages.pricing.noMatchBody")}
              actions={
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("common.clearFilters")}
                </Button>
              }
            />
          ) : (
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
                    setSeedModel("");
                    setEditOpen(true);
                  }}
                >
                  {t("pages.pricing.emptyAction")}
                </GatedButton>
              }
            />
          )}
        </ListEmptyRow>
      </ListTable>

      <UpsertPriceDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        existing={editTarget}
        seedModel={seedModel}
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
                detail: errorText(error, t),
              }),
          });
        }}
      />
    </PageBody>
  );
}

/**
 * The models that carried traffic with no price row (#2102): the reason this
 * screen exists, so it names them rather than leaving the reader to find the
 * flag row by row in LLM Logs. The count is the by-model rollup's
 * `unpriced_requests`, which the control plane scales by each row's sample
 * weight. A model priced since the window began is left out, and the list is
 * absent when nothing is unpriced.
 */
function UnpricedModels({
  priced,
  onAdd,
}: {
  priced: UseQueryResult<ModelPriceRow[]>;
  onAdd: (model: string) => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const traffic = useQuery({
    queryKey: ["analytics", "by-model", "unpriced", UNPRICED_WINDOW],
    queryFn: () => fetchAnalyticsByModel(windowBounds(UNPRICED_WINDOW)),
    retry: false,
  });
  const pricedModels = new Set((priced.data ?? []).map((p) => p.model));
  const rows = (traffic.data ?? [])
    .map((r) => ({ model: r.model, requests: Number(r.unpriced_requests) }))
    .filter((r) => r.model && r.requests > 0 && !pricedModels.has(r.model))
    .sort((a, b) => b.requests - a.requests);

  // without ClickHouse there is no traffic to name; say so rather than let an
  // absent list read as "nothing is missing"
  if (traffic.error instanceof AnalyticsUnavailableError) {
    return <AnalyticsUnavailable error={traffic.error} i18nKey="pages.pricing.noAnalytics" />;
  }
  if (traffic.error) {
    return (
      // load-error-allow: the read only decorates the catalog below, which owns the region's empty state
      <LoadError
        error={traffic.error}
        resource={t("errors.resources.unpricedModels")}
        onRetry={() => traffic.refetch()}
        target="unpriced-models"
      />
    );
  }
  if (traffic.isLoading || priced.isLoading) {
    return <ListSkeleton rows={2} className="p-3" />;
  }
  if (rows.length === 0) return null;

  return (
    <section aria-labelledby="unpriced-models" className="flex flex-col gap-2">
      <div>
        <h2 id="unpriced-models" className="text-sm font-semibold">
          {t("pages.pricing.unpriced.title", { count: rows.length })}
        </h2>
        <p className="text-xs text-muted-foreground">{t("pages.pricing.unpriced.body")}</p>
      </div>
      <SampledLogNotice kind="figures" />
      <ListTable label={t("pages.pricing.unpriced.tableLabel")} minWidth={560}>
        <ListHeader grid={UNPRICED_GRID}>
          <ListHeaderCell>{t("pages.pricing.columns.model")}</ListHeaderCell>
          <ListHeaderCell className="text-right">
            {t("pages.pricing.unpriced.requests")}
          </ListHeaderCell>
          <ListActionsHeader className={STICKY_ACTIONS.header} />
        </ListHeader>
        {rows.map((r) => (
          <ListRow key={r.model} grid={UNPRICED_GRID}>
            <ListCell className="truncate font-mono text-sm font-semibold">{r.model}</ListCell>
            <ListCell className="text-right font-mono text-sm tabular-nums">
              {fmt.number(r.requests)}
            </ListCell>
            <ListCell className={`flex items-center justify-end ${STICKY_ACTIONS.cell}`}>
              <GatedButton
                gate="model_price:update"
                control="price-add-unpriced"
                size="sm"
                variant="outline"
                aria-label={t("pages.pricing.unpriced.addAria", { model: r.model })}
                onClick={() => onAdd(r.model)}
              >
                {t("pages.pricing.emptyAction")}
              </GatedButton>
            </ListCell>
          </ListRow>
        ))}
      </ListTable>
    </section>
  );
}

/** the fields of the price form, for the errors a refused save puts on screen */
type PriceField =
  "model" | "input" | "output" | "cached" | "cacheWrite" | "cacheWrite1h" | "currency";

/**
 * The rate inputs a rejected save can be pinned to (#2902). A negative or
 * non-numeric rate is refused before the request is sent; what only the control
 * plane knows is a rate the stored column cannot hold, so that refusal is placed
 * on its input instead of surfacing as a toast.
 */
const WIRE_FIELDS: Record<string, "cacheWrite" | "cacheWrite1h"> = {
  cache_write_per_mtok: "cacheWrite",
  cache_write_1h_per_mtok: "cacheWrite1h",
};

function UpsertPriceDialog({
  open,
  onOpenChange,
  existing,
  seedModel,
  currency: settings,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing: ModelPriceRow | null;
  /** the model a new price opens with, from the unpriced list; empty for a blank form */
  seedModel: string;
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
  const [cacheWritePerMtok, setCacheWritePerMtok] = React.useState("");
  const [cacheWrite1hPerMtok, setCacheWrite1hPerMtok] = React.useState("");
  const baseCurrency = settings?.base ?? "USD";
  const [currency, setCurrency] = React.useState(baseCurrency);
  const visibility = useErrorVisibility<PriceField>();
  const idBase = React.useId();
  const writeIds = {
    cacheWrite: `${idBase}-cache-write`,
    cacheWrite1h: `${idBase}-cache-write-1h`,
  };

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
      visibility.reset();
      setModel(existing?.model ?? seedModel);
      setInputPerMtok(existing?.input_per_mtok ?? "");
      setOutputPerMtok(existing?.output_per_mtok ?? "");
      setCachedInputPerMtok(existing?.cached_input_per_mtok ?? "");
      setCacheWritePerMtok(existing?.cache_write_per_mtok ?? "");
      setCacheWrite1hPerMtok(existing?.cache_write_1h_per_mtok ?? "");
      fields.clear();
      setCurrency(existing?.currency ?? baseCurrency);
    }
  }, [open, existing, seedModel]);

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
    cacheWrite: cacheWritePerMtok.trim() ? priceError(cacheWritePerMtok) : undefined,
    cacheWrite1h: cacheWrite1hPerMtok.trim() ? priceError(cacheWrite1hPerMtok) : undefined,
    currency: currency.trim() ? undefined : t("pages.pricing.currencyRequired"),
  };
  const fields = useFieldErrors<PriceField>({
    order: ["model", "input", "output", "cached", "cacheWrite", "cacheWrite1h", "currency"],
    wire: WIRE_FIELDS,
    idOf: (field) => (field === "cacheWrite" || field === "cacheWrite1h" ? writeIds[field] : field),
    local: errors,
    // a field's error waits for a refused save, so the form opens with none
    shows: visibility.shows,
  });

  const submit = useMutation({
    mutationFn: () =>
      upsertModelPrice({
        model,
        input_per_mtok: inputPerMtok,
        output_per_mtok: outputPerMtok,
        cached_input_per_mtok: cachedInputPerMtok.trim() || undefined,
        // the write rate is the one a save does not replace: it goes only when
        // it was edited, a value to set it and null to clear it (#2876)
        ...cacheWritePatch(cacheWritePerMtok, existing?.cache_write_per_mtok ?? ""),
        ...cacheWrite1hPatch(cacheWrite1hPerMtok, existing?.cache_write_1h_per_mtok ?? ""),
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
      // a refusal that names one of the rate inputs stays on that input, with
      // the sheet still open and focus on it, rather than in a toast
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: model }),
        detail: errorText(error, t),
      });
    },
  });

  const invalid = Object.values(errors).some(Boolean);
  const shown = fields.errorOf;

  const dirty =
    model.trim() !== (existing?.model ?? seedModel) ||
    inputPerMtok !== (existing?.input_per_mtok ?? "") ||
    outputPerMtok !== (existing?.output_per_mtok ?? "") ||
    cachedInputPerMtok !== (existing?.cached_input_per_mtok ?? "") ||
    cacheWritePerMtok !== (existing?.cache_write_per_mtok ?? "") ||
    cacheWrite1hPerMtok !== (existing?.cache_write_1h_per_mtok ?? "") ||
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
      errorMessage={
        submit.isError && !fields.names(submit.error) ? errorText(submit.error, t) : undefined
      }
      saveLabel={t("common.save")}
      canSave
      saving={submit.isPending}
      onSave={() => {
        // the refusal is shown at the fields, not by greying the button out
        visibility.attempt();
        if (!invalid) submit.mutate();
      }}
    >
      <div className="space-y-3">
        <Field label={t("pages.pricing.modelName")} error={shown("model")}>
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
          error={shown("input")}
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
          error={shown("output")}
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
          error={shown("cached")}
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
          label={t("pages.pricing.cacheWriteInputPrice")}
          hint={t("pages.pricing.priceUnit", { currency })}
          info={t("pages.pricing.cacheWriteInfo")}
          error={shown("cacheWrite")}
        >
          <Input
            id={writeIds.cacheWrite}
            type="number"
            min={0}
            step="0.000001"
            value={cacheWritePerMtok}
            onChange={(e) => {
              setCacheWritePerMtok(e.target.value);
              fields.edit("cacheWrite");
            }}
            placeholder={t("pages.pricing.cacheWritePlaceholder")}
          />
        </Field>
        <Field
          label={t("pages.pricing.cacheWrite1hInputPrice")}
          hint={t("pages.pricing.priceUnit", { currency })}
          info={t("pages.pricing.cacheWrite1hInfo")}
          error={shown("cacheWrite1h")}
        >
          <Input
            id={writeIds.cacheWrite1h}
            type="number"
            min={0}
            step="0.000001"
            value={cacheWrite1hPerMtok}
            onChange={(e) => {
              setCacheWrite1hPerMtok(e.target.value);
              fields.edit("cacheWrite1h");
            }}
            placeholder={t("pages.pricing.cacheWrite1hPlaceholder")}
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
          error={shown("currency")}
        >
          <Combobox allowCustom options={currencyOptions} value={currency} onChange={setCurrency} />
        </Field>
      </div>
    </EditorSheet>
  );
}
