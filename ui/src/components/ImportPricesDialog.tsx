import { useMutation, useQuery } from "@tanstack/react-query";
import { CircleDollarSign, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { SearchInput } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import {
  fetchPriceCatalogue,
  importModelPrices,
  type CatalogueEntry,
  type ImportOutcome,
  type ModelPriceRow,
  type PriceImportResult,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { useFormTelemetry } from "@/lib/ux-react";

// how many matches one query asks for: the snapshot holds thousands of models,
// so the list is a search result and never the whole catalogue
const CATALOGUE_LIMIT = 50;
const DEBOUNCE_MS = 250;

// the order a summary reads in: what changed first, what was left alone after
const OUTCOMES: ImportOutcome[] = ["created", "updated", "kept_override", "not_in_catalogue"];

/** the value after it has stopped changing for `ms` */
function useDebounced<T>(value: T, ms: number): T {
  const [settled, setSettled] = React.useState(value);
  React.useEffect(() => {
    const timer = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return settled;
}

/**
 * Copy prices from the vendored catalogue snapshot (#966). The snapshot ships
 * with the control plane and is never fetched at runtime; the choice of source
 * is provisional, so the dialog says on every open that these are third-party
 * list prices, subject to change, and names the source, licence and snapshot
 * date. A price an operator already set is never replaced: the result reports
 * it as kept.
 */
export function ImportPricesDialog({
  open,
  onOpenChange,
  existing,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the stored prices, to mark a model whose own price the import would keep */
  existing: ModelPriceRow[] | undefined;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const [search, setSearch] = React.useState("");
  const [picked, setPicked] = React.useState<string[]>([]);
  const q = useDebounced(search.trim(), DEBOUNCE_MS);

  const catalogue = useQuery({
    queryKey: ["price-catalogue", q],
    queryFn: () => fetchPriceCatalogue(q, CATALOGUE_LIMIT),
    enabled: open,
    // the snapshot is fixed for the life of a control plane
    staleTime: Infinity,
    retry: false,
    placeholderData: (previous) => previous,
  });

  const run = useMutation({
    mutationFn: (models: string[]) => importModelPrices(models),
    onSuccess: onDone,
  });
  const reset = run.reset;
  React.useEffect(() => {
    if (open) {
      setSearch("");
      setPicked([]);
      reset();
    }
  }, [open, reset]);

  const ux = useFormTelemetry("price-catalogue-import", open, { dirty: picked.length > 0 });
  const overrides = new Set((existing ?? []).filter((p) => p.source === null).map((p) => p.model));
  const toggle = (model: string) =>
    setPicked((now) => (now.includes(model) ? now.filter((m) => m !== model) : [...now, model]));
  const result = run.data;
  const data = catalogue.data;
  const day = data ? fmt.date(data.snapshot, { dateStyle: "medium", timeZone: "UTC" }) : null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg">
      <DialogHeader>
        <DialogTitle>{t("pages.pricing.import.title")}</DialogTitle>
        <DialogDescription>{t("pages.pricing.import.body")}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <div className="space-y-3">
          <p
            role="note"
            className="rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 text-xs"
          >
            <strong className="font-semibold">{t("pages.pricing.import.noticeLead")}</strong>{" "}
            {t("pages.pricing.import.notice")}
          </p>
          {data && day && (
            <p className="text-xs text-muted-foreground">
              {t("pages.pricing.import.sourceLine", {
                source: data.source,
                licence: data.licence,
                date: day,
                total: fmt.number(data.total),
              })}
            </p>
          )}
          <SearchInput
            className="max-w-none"
            placeholder={t("pages.pricing.import.searchPlaceholder")}
            aria-label={t("pages.pricing.import.searchPlaceholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {catalogue.isLoading ? (
            <ListSkeleton rows={4} />
          ) : catalogue.error ? (
            <LoadError
              error={catalogue.error}
              resource={t("errors.resources.priceCatalogue")}
              onRetry={() => catalogue.refetch()}
              target="price-catalogue"
            />
          ) : data && data.entries.length === 0 ? (
            <EmptyState
              uxTarget="price-catalogue"
              icon={<CircleDollarSign />}
              title={t("pages.pricing.import.noMatchTitle")}
              description={
                q
                  ? t("pages.pricing.import.noMatchBody", { query: q })
                  : t("pages.pricing.import.emptyBody")
              }
            />
          ) : (
            data && (
              <ul
                aria-label={t("pages.pricing.import.listLabel")}
                className="max-h-72 divide-y divide-[color:var(--border-subtle)] overflow-y-auto rounded-md border border-[color:var(--border-subtle)]"
              >
                {data.entries.map((entry) => (
                  <CatalogueRow
                    key={entry.model}
                    entry={entry}
                    checked={picked.includes(entry.model)}
                    isOverride={overrides.has(entry.model)}
                    onToggle={() => toggle(entry.model)}
                  />
                ))}
              </ul>
            )
          )}
          {data && data.total > data.entries.length && data.entries.length > 0 && (
            <p className="text-xs text-muted-foreground">
              {t("pages.pricing.import.truncated", { shown: data.entries.length })}
            </p>
          )}
          <div aria-live="polite" className="space-y-2">
            {result && <ImportSummary result={result} />}
            {run.isError && (
              <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
                {errorText(run.error, t)}
              </p>
            )}
          </div>
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          {t("common.close")}
        </Button>
        <GatedButton
          gate="model_price:update"
          control="price-import-run"
          disabled={run.isPending || picked.length === 0}
          onClick={() => {
            ux.submitted();
            run.mutate(picked, {
              onSuccess: () => {
                ux.saved();
                setPicked([]);
              },
              onError: () => ux.failed(),
            });
          }}
        >
          {run.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
          {t("pages.pricing.import.run", { n: picked.length })}
        </GatedButton>
      </DialogFooter>
    </Dialog>
  );
}

function CatalogueRow({
  entry,
  checked,
  isOverride,
  onToggle,
}: {
  entry: CatalogueEntry;
  checked: boolean;
  isOverride: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const rate = (value: string | null) =>
    value == null
      ? "—"
      : fmt.number(Number(value), { minimumFractionDigits: 2, maximumFractionDigits: 6 });
  return (
    <li>
      <label className="flex cursor-pointer items-start gap-3 p-2.5 text-sm hover:bg-muted/40">
        <Checkbox checked={checked} onChange={onToggle} className="mt-0.5" />
        <span className="min-w-0 flex-1">
          <span className="block truncate font-mono font-semibold">{entry.model}</span>
          <span className="block font-mono text-xs tabular-nums text-muted-foreground">
            {t("pages.pricing.import.rates", {
              input: rate(entry.input_per_mtok),
              output: rate(entry.output_per_mtok),
              cached: rate(entry.cached_input_per_mtok),
              currency: entry.currency,
            })}
          </span>
        </span>
        {isOverride && (
          <Badge tone="neutral" title={t("pages.pricing.import.overrideKept")}>
            {t("priceProvenance.override")}
          </Badge>
        )}
      </label>
    </li>
  );
}

/** what an import did, one line per outcome that happened, naming the models */
function ImportSummary({ result }: { result: PriceImportResult }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const day = fmt.date(result.snapshot, { dateStyle: "medium", timeZone: "UTC" });
  return (
    <div
      role="status"
      className="space-y-1 rounded-md border border-[color:var(--border-subtle)] p-3 text-xs"
    >
      <p className="font-semibold">
        {t("pages.pricing.import.resultTitle", { source: result.source, date: day })}
      </p>
      <ul className="space-y-0.5">
        {OUTCOMES.map((outcome) => {
          const models = result.results.filter((r) => r.outcome === outcome).map((r) => r.model);
          if (models.length === 0) return null;
          return (
            <li key={outcome}>
              <span className="font-medium">{t(`pages.pricing.import.outcome.${outcome}`)}</span>{" "}
              <span className="font-mono text-muted-foreground">{models.join(", ")}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
