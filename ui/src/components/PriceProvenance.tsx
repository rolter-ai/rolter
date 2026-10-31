import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import type { ModelPriceRow } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";

/**
 * Where a stored price came from (#966). A price with a `source` was imported
 * from the vendored catalogue snapshot and is a third-party list price, subject
 * to change; a price without one was set by an operator, wins over the
 * catalogue, and is an override. A row that carries no `source` key at all
 * (a control plane that predates the import) says nothing either way.
 */
export function priceProvenance(
  price: Pick<ModelPriceRow, "source" | "source_imported_at"> | undefined,
): { kind: "catalogue"; source: string; snapshot: string | null } | { kind: "override" } | null {
  if (!price || price.source === undefined) return null;
  if (price.source === null) return { kind: "override" };
  return { kind: "catalogue", source: price.source, snapshot: price.source_imported_at ?? null };
}

/** the snapshot's day, written in the dashboard's locale; the day is what the snapshot is dated by */
function useSnapshotDay() {
  const fmt = useFormat();
  return (value: string | null) =>
    value ? fmt.date(value, { dateStyle: "medium", timeZone: "UTC" }) : null;
}

/**
 * The marker on a price row: "Catalogue" for an imported price, with the source
 * and snapshot date in the title, "Override" for one an operator set.
 */
export function PriceProvenanceBadge({
  price,
  className,
}: {
  price: Pick<ModelPriceRow, "source" | "source_imported_at">;
  className?: string;
}) {
  const { t } = useTranslation();
  const day = useSnapshotDay();
  const provenance = priceProvenance(price);
  if (!provenance) return null;
  if (provenance.kind === "override") {
    return (
      <Badge tone="neutral" className={className} title={t("priceProvenance.overrideHint")}>
        {t("priceProvenance.override")}
      </Badge>
    );
  }
  const snapshot = day(provenance.snapshot);
  const lines = [
    t("priceProvenance.catalogueSource", { source: provenance.source }),
    snapshot ? t("priceProvenance.catalogueSnapshot", { date: snapshot }) : null,
    t("priceProvenance.catalogueHint"),
  ].filter((line): line is string => !!line);
  return (
    <Badge tone="info" className={className} title={lines.join("\n")}>
      {t("priceProvenance.catalogue")}
    </Badge>
  );
}

/**
 * The provenance of a model's price as a sentence, for the sheet that edits it.
 * Renders nothing for a model with no price, or one whose provenance is unknown.
 */
export function PriceProvenanceLine({
  price,
  className,
}: {
  price: Pick<ModelPriceRow, "source" | "source_imported_at"> | undefined;
  className?: string;
}) {
  const { t } = useTranslation();
  const day = useSnapshotDay();
  const provenance = priceProvenance(price);
  if (!provenance) return null;
  const snapshot = provenance.kind === "catalogue" ? day(provenance.snapshot) : null;
  return (
    <p className={className ?? "text-xs text-muted-foreground"}>
      {provenance.kind === "override"
        ? t("priceProvenance.lineOverride")
        : snapshot
          ? t("priceProvenance.lineCatalogue", { source: provenance.source, date: snapshot })
          : t("priceProvenance.lineCatalogueUndated", { source: provenance.source })}
    </p>
  );
}
