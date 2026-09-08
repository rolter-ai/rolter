import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleDollarSign, Plus, Trash2, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { useGate } from "@/lib/can";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import { PageBody, Toolbar } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  deleteModelPrice,
  fetchCurrencySettings,
  fetchModelPrices,
  isConvertible,
  upsertModelPrice,
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

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: PRICES_QUERY_KEY });

  const removePrice = useMutation({
    mutationFn: (model: string) => deleteModelPrice(model),
    onSuccess: invalidate,
  });

  const [editOpen, setEditOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<ModelPriceRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<ModelPriceRow | null>(null);
  // model prices are deployment-wide, so a row control is the superadmin's
  // exactly as the add button is (#1258)
  const deleteGate = useGate("model_price:delete");

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {prices.data?.length ?? 0} models · per-million-token pricing · currency set per model
        </span>
        {/* a price is written with PUT /model-prices whether or not the row
            exists, so adding one takes `model_price:update` — there is no
            create capability to gate on (#1258) */}
        <GatedButton
          gate="model_price:update"
          className="ml-auto"
          onClick={() => {
            setEditTarget(null);
            setEditOpen(true);
          }}
        >
          <Plus className="h-4 w-4" />
          Add price
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
          <div
            key={price.id}
            className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
          >
            <div className="truncate font-mono text-sm font-semibold">{price.model}</div>
            <div className="flex flex-wrap gap-1.5">
              <Badge tone="outline">
                in {price.input_per_mtok} {price.currency}/Mtok
              </Badge>
              <Badge tone="outline">
                out {price.output_per_mtok} {price.currency}/Mtok
              </Badge>
              {price.cached_input_per_mtok && (
                <Badge tone="neutral">
                  cached {price.cached_input_per_mtok} {price.currency}/Mtok
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
                size="sm"
                variant="outline"
                aria-label={t("pages.pricing.editAria", { model: price.model })}
                onClick={() => {
                  setEditTarget(price);
                  setEditOpen(true);
                }}
              >
                Edit
              </GatedButton>
              <button
                type="button"
                title={
                  deleteGate.reason ??
                  t("pages.pricing.deleteAria", { model: price.model })
                }
                aria-label={t("pages.pricing.deleteAria", { model: price.model })}
                disabled={
                  deleteGate.denied ||
                  (removePrice.isPending && deleteTarget?.model === price.model)
                }
                onClick={() => setDeleteTarget(price)}
                className="flex items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {removePrice.isPending && deleteTarget?.model === price.model ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
              </button>
            </div>
          </div>
        ))}
      </div>

      <UpsertPriceDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        existing={editTarget}
        onDone={invalidate}
      />

      <Dialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <DialogHeader>
          <DialogTitle>Delete price</DialogTitle>
          <DialogDescription>
            Removes the pricing entry for{" "}
            <span className="font-mono">{deleteTarget?.model}</span>. Cost
            accounting for this model falls back to no known price.
          </DialogDescription>
        </DialogHeader>
        {removePrice.isError && (
          <p className="text-xs text-[color:var(--status-danger-text)]">
            {(removePrice.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removePrice.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              const what = deleteTarget.model;
              removePrice.mutate(what, {
                onSuccess: () => {
                  setDeleteTarget(null);
                  toast.push({ tone: "success", title: t("toast.deleted", { what }) });
                },
                onError: (error) => {
                  toast.push({
                    tone: "error",
                    title: t("toast.deleteFailed", { what }),
                    detail: errorDetail(error),
                  });
                },
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}

function UpsertPriceDialog({
  open,
  onOpenChange,
  existing,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing: ModelPriceRow | null;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [model, setModel] = React.useState("");
  const [inputPerMtok, setInputPerMtok] = React.useState("0");
  const [outputPerMtok, setOutputPerMtok] = React.useState("0");
  const [cachedInputPerMtok, setCachedInputPerMtok] = React.useState("");
  const [currency, setCurrency] = React.useState("USD");

  React.useEffect(() => {
    if (open) {
      setModel(existing?.model ?? "");
      setInputPerMtok(existing?.input_per_mtok ?? "0");
      setOutputPerMtok(existing?.output_per_mtok ?? "0");
      setCachedInputPerMtok(existing?.cached_input_per_mtok ?? "");
      setCurrency(existing?.currency ?? "USD");
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

  const dirty =
    model.trim() !== (existing?.model ?? "") ||
    inputPerMtok !== (existing?.input_per_mtok ?? "0") ||
    outputPerMtok !== (existing?.output_per_mtok ?? "0") ||
    cachedInputPerMtok !== (existing?.cached_input_per_mtok ?? "") ||
    currency !== (existing?.currency ?? "USD");

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title={existing ? `Edit ${existing.model}` : "Add price"}
      subtitle="Prices are per million tokens (Mtok); saving upserts by model name."
      dirty={dirty}
      errorMessage={submit.isError ? (submit.error as Error).message : undefined}
      saveLabel="Save"
      canSave={!!model.trim() && !!inputPerMtok.trim() && !!outputPerMtok.trim()}
      saving={submit.isPending}
      onSave={() => submit.mutate()}
    >
      <div className="space-y-3">
        <Field label="Model name">
          <Input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="gpt-4o"
            disabled={!!existing}
          />
        </Field>
        <Field label="Input price per Mtok">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={inputPerMtok}
            onChange={(e) => setInputPerMtok(e.target.value)}
          />
        </Field>
        <Field label="Output price per Mtok">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={outputPerMtok}
            onChange={(e) => setOutputPerMtok(e.target.value)}
          />
        </Field>
        <Field label="Cached input price per Mtok (optional)">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={cachedInputPerMtok}
            onChange={(e) => setCachedInputPerMtok(e.target.value)}
            placeholder="defaults to input price"
          />
        </Field>
        <Field
          label="Currency"
          hint="Any code — ISO-4217, crypto, or a custom unit. Anything other than the base currency needs a rate in [currency.rates]; without one the price is rejected rather than charged at the wrong rate."
          info="Spend and budgets accumulate in the deployment's base currency. A price in another currency is converted at the configured rate before it reaches a budget."
        >
          <Input value={currency} onChange={(e) => setCurrency(e.target.value)} />
        </Field>
      </div>
    </EditorSheet>
  );
}
