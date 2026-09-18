import { Building2, WalletCards } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { FilterCheckList } from "@/components/ui/filter-panel";
import type { BusinessUnitRow, CustomerRow, ProviderRow } from "@/lib/api";

// where a key's spend is charged, and how far it can reach (#1193).
//
// Both screens that own a virtual key need the same three controls — the admin
// editor on Keys and the self-service mint on Account — and the two icons used
// here are the same ones the Business Units and Customers screens carry, so the
// building always means "rolls up inside the org" and the wallet always means
// "bills out to someone we serve", wherever it appears.

/** the select's "no attribution" token; its own value so an unset control can
 *  never be mistaken for a deliberate choice */
export const UNATTRIBUTED = "__unattributed__";

/** a select value back to the wire's three-state contract: `null` clears */
export function attributionId(value: string): string | null {
  return value === UNATTRIBUTED ? null : value;
}

/** a stored id back to a select value */
export function attributionValue(id: string | null | undefined): string {
  return id ?? UNATTRIBUTED;
}

/**
 * Business unit + customer, as the server pairs them.
 *
 * The control plane rejects a customer already owned by a *different* business
 * unit, so a customer that cannot be paired with the chosen unit is not offered
 * at all — learning that rule from a 400 after saving is learning it too late.
 * Retired entities are dropped for the same reason they are dropped from the
 * Customers screen's own dropdown: they keep their history and stop taking new
 * attribution, unless they are already the selection being edited.
 */
export function KeyAttributionFields({
  units,
  customers,
  businessUnitId,
  customerId,
  onChange,
}: {
  units: BusinessUnitRow[];
  customers: CustomerRow[];
  businessUnitId: string;
  customerId: string;
  onChange: (businessUnitId: string, customerId: string) => void;
}) {
  const { t } = useTranslation();
  const offered = customers.filter((c) => {
    // never hide the pick being edited, however it was made
    if (c.id === customerId) return true;
    if (c.retired_at) return false;
    if (businessUnitId === UNATTRIBUTED) return true;
    return !c.business_unit_id || c.business_unit_id === businessUnitId;
  });
  // moving the unit can strand the customer under it; the pairing rule lives
  // here rather than in each caller, so both editors drop it the same way
  const pickUnit = (unit: string) => {
    const owner = customers.find((c) => c.id === customerId)?.business_unit_id;
    const stranded =
      customerId !== UNATTRIBUTED && unit !== UNATTRIBUTED && !!owner && owner !== unit;
    onChange(unit, stranded ? UNATTRIBUTED : customerId);
  };
  return (
    <>
      <Field label={t("keyMint.businessUnit")} hint={t("keyMint.businessUnitHint")}>
        <Combobox
          aria-label={t("keyMint.businessUnit")}
          value={businessUnitId}
          onChange={pickUnit}
          options={[
            { value: UNATTRIBUTED, label: t("keyMint.unattributed") },
            ...units
              .filter((u) => !u.retired_at || u.id === businessUnitId)
              .map((u) => ({ value: u.id, label: u.name })),
          ]}
        />
      </Field>
      <Field label={t("keyMint.customer")} hint={t("keyMint.customerHint")}>
        <Combobox
          aria-label={t("keyMint.customer")}
          value={customerId}
          onChange={(customer) => onChange(businessUnitId, customer)}
          options={[
            { value: UNATTRIBUTED, label: t("keyMint.unattributed") },
            ...offered.map((c) => ({ value: c.id, label: c.name })),
          ]}
        />
      </Field>
    </>
  );
}

/**
 * The upstream providers a key may reach, by slug.
 *
 * An empty selection is the permissive default the server documents, so the
 * hint says so out loud: an operator who ticks nothing has not locked the key
 * out of everything, and the opposite reading is the expensive one.
 */
export function KeyProvidersField({
  providers,
  selected,
  onChange,
}: {
  providers: ProviderRow[];
  selected: string[];
  onChange: (selected: string[]) => void;
}) {
  const { t } = useTranslation();
  if (providers.length === 0) return null;
  return (
    <Field
      label={t("keyMint.providers")}
      hint={
        selected.length === 0
          ? t("keyMint.providersAll")
          : t("keyMint.providersSome", { count: selected.length })
      }
    >
      <FilterCheckList
        options={providers.map((p) => ({ value: p.slug, label: p.name }))}
        selected={selected}
        onChange={onChange}
      />
    </Field>
  );
}

/**
 * Where a key's spend lands, on a list row.
 *
 * An unattributed key renders nothing rather than an "unattributed" chip:
 * attribution is optional, and badging its absence on every row would read as a
 * warning about a state most deployments are legitimately in.
 */
export function AttributionBadges({ unit, customer }: { unit?: string; customer?: string }) {
  const { t } = useTranslation();
  if (!unit && !customer) {
    return (
      <span className="text-[color:var(--text-subtle)]" title={t("keyMint.unattributed")}>
        &mdash;
      </span>
    );
  }
  return (
    <div className="flex min-w-0 flex-wrap gap-1">
      {unit && (
        <Badge tone="outline" title={t("keyMint.businessUnit")}>
          <Building2 className="h-3 w-3" />
          <span className="min-w-0 truncate">{unit}</span>
        </Badge>
      )}
      {customer && (
        <Badge tone="accent" title={t("keyMint.customer")}>
          <WalletCards className="h-3 w-3" />
          <span className="min-w-0 truncate">{customer}</span>
        </Badge>
      )}
    </div>
  );
}
