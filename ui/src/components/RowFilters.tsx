import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { Combobox } from "@/components/ui/combobox";
import { FilterSearchList, FilterSection } from "@/components/ui/filter-panel";
import {
  fetchBusinessUnits,
  fetchCustomers,
  fetchModels,
  fetchVirtualKeys,
  type AnalyticsRowFilters,
  type SavedViewFilters,
} from "@/lib/api";
import { useCan } from "@/lib/can";
import { useScope } from "@/lib/scope";
import {
  windowAddress,
  windowFromFilters,
  type TIME_WINDOW_PARAM,
  type WINDOW_SINCE_PARAM,
  type WINDOW_UNTIL_PARAM,
} from "@/lib/time-window";

// the four row filters LLM Logs and the Dashboard share (#2453, #2725): model,
// virtual key, business unit and customer. the control plane applies them with
// one predicate on every analytics read, so a filter picks the same rows on
// both screens. they live in the address under the control plane's own names,
// so a view can be reloaded, bookmarked or pasted

/** the address parameters the four filters use */
export const ROW_FILTER_PARAMS = ["model", "key", "business_unit", "customer"] as const;
export type RowFilterParam = (typeof ROW_FILTER_PARAMS)[number];

/** the lists the pickers offer, loaded once per screen */
export function useRowFilterData() {
  const scope = useScope();
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  // the two governance dimensions a row can be attributed to. named so the
  // rail and a drawer both show "Platform Engineering" rather than the uuid
  // ClickHouse actually stores
  const units = useQuery({
    queryKey: ["business-units", scope.orgId],
    queryFn: () => fetchBusinessUnits(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  // the keys the caller can read in the scoped project (#1983). a caller the
  // gate refuses gets no picker rather than a list that always fails
  const can = useCan();
  const keys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: can("virtual_key", "read") !== false && !!scope.projectId,
    retry: false,
    staleTime: 60_000,
  });
  return { models, units, customers, keys };
}

export type RowFilterData = ReturnType<typeof useRowFilterData>;

/** the selection the pickers show */
export interface RowFilterValues {
  model: string;
  keyId: string;
  units: string[];
  customers: string[];
}

export interface RowFilterSetters {
  setModel: (next: string) => void;
  setKey: (next: string) => void;
  setUnits: (next: string[]) => void;
  setCustomers: (next: string[]) => void;
}

/**
 * The four pickers as rail sections: model and key are one exact value each,
 * business unit and customer a set. A model or key the address names but the
 * lists no longer hold still filters, so it is offered too: otherwise the
 * control would read as unset while the rows are narrowed.
 */
export function RowFilterSections({
  data,
  values,
  setters,
  openAll = false,
}: {
  data: RowFilterData;
  values: RowFilterValues;
  setters: RowFilterSetters;
  /** every section starts open, for a panel with room for all four */
  openAll?: boolean;
}) {
  const { t } = useTranslation();
  const { model, keyId, units: unitSel, customers: customerSel } = values;
  const modelOptions = React.useMemo(() => {
    const names = (data.models.data ?? []).map((m) => m.model);
    if (model && !names.includes(model)) names.push(model);
    return names.map((name) => ({ value: name, label: name }));
  }, [data.models.data, model]);
  // by name and prefix; an id a saved view or pasted link names that this
  // project's list does not hold is offered by the id
  const keyOptions = React.useMemo(() => {
    const options = (data.keys.data ?? []).map((k) => ({
      value: k.id,
      label: k.name ? `${k.name} (${k.key_prefix}…)` : `${k.key_prefix}…`,
    }));
    if (keyId && !options.some((o) => o.value === keyId)) {
      options.push({ value: keyId, label: keyId });
    }
    return options;
  }, [data.keys.data, keyId]);
  return (
    <>
      {/* the control plane filters on one exact model, so this picks one */}
      <FilterSection title={t("pages.logs.model")} defaultOpen count={model ? 1 : 0}>
        <Combobox
          aria-label={t("pages.logs.model")}
          options={modelOptions}
          value={model}
          onChange={setters.setModel}
          placeholder={t("pages.logs.allModels")}
          clearable
        />
      </FilterSection>
      {/* one exact key id, shown by its name */}
      {keyOptions.length > 0 && (
        <FilterSection title={t("pages.logs.virtualKey")} defaultOpen count={keyId ? 1 : 0}>
          <Combobox
            aria-label={t("pages.logs.virtualKey")}
            options={keyOptions}
            value={keyId}
            onChange={setters.setKey}
            placeholder={t("pages.logs.allKeys")}
            clearable
          />
        </FilterSection>
      )}
      {(data.units.data ?? []).length > 0 && (
        <FilterSection
          title={t("pages.logs.businessUnit")}
          defaultOpen={openAll}
          count={unitSel.length}
        >
          <FilterSearchList
            options={(data.units.data ?? []).map((u) => ({ value: u.id, label: u.name }))}
            selected={unitSel}
            onChange={setters.setUnits}
            placeholder={t("pages.logs.filterBusinessUnits")}
          />
        </FilterSection>
      )}
      {(data.customers.data ?? []).length > 0 && (
        <FilterSection
          title={t("pages.logs.customer")}
          defaultOpen={openAll}
          count={customerSel.length}
        >
          <FilterSearchList
            options={(data.customers.data ?? []).map((c) => ({ value: c.id, label: c.name }))}
            selected={customerSel}
            onChange={setters.setCustomers}
            placeholder={t("pages.logs.filterCustomers")}
          />
        </FilterSection>
      )}
    </>
  );
}

/**
 * The pickers with their own lists, for a panel that is only sometimes drawn:
 * the lists are read when it mounts, so a screen that never opens it asks for
 * none of them.
 */
export function RowFilterPicker({
  values,
  setters,
}: {
  values: RowFilterValues;
  setters: RowFilterSetters;
}) {
  const data = useRowFilterData();
  return <RowFilterSections data={data} values={values} setters={setters} openAll />;
}

/**
 * The four filters as the Dashboard keeps them: in the address, every write
 * replacing the history entry. Other parameters (`window`) are left alone.
 */
export function useAddressRowFilters() {
  const [params, setParams] = useSearchParams();
  const model = params.get("model") ?? "";
  const keyId = (params.get("key") ?? "").trim();
  const unitParam = params.get("business_unit") ?? "";
  const customerParam = params.get("customer") ?? "";
  // memoised on the raw value: a fresh array every render would look like a
  // changed filter to anything that depends on it
  const units = React.useMemo(() => unitParam.split(",").filter(Boolean), [unitParam]);
  const customers = React.useMemo(() => customerParam.split(",").filter(Boolean), [customerParam]);
  const update = React.useCallback(
    (
      patch: Partial<
        Record<
          | RowFilterParam
          | typeof TIME_WINDOW_PARAM
          | typeof WINDOW_SINCE_PARAM
          | typeof WINDOW_UNTIL_PARAM,
          string
        >
      >,
    ) =>
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          for (const [name, value] of Object.entries(patch)) {
            if (value) next.set(name, value);
            else next.delete(name);
          }
          return next;
        },
        { replace: true },
      ),
    [setParams],
  );
  const query = React.useMemo<AnalyticsRowFilters>(
    () => ({
      model: model || undefined,
      key: keyId || undefined,
      business_unit: units.length ? units : undefined,
      customer: customers.length ? customers : undefined,
    }),
    [model, keyId, units, customers],
  );
  const count = (model ? 1 : 0) + (keyId ? 1 : 0) + units.length + customers.length;
  return {
    values: { model, keyId, units, customers } satisfies RowFilterValues,
    /** what the reads send; empty entries are left out */
    query,
    /** what a saved view keeps, with empty values left out */
    saved: {
      ...(model ? { model } : {}),
      ...(keyId ? { key: keyId } : {}),
      ...(units.length ? { business_unit: units } : {}),
      ...(customers.length ? { customer: customers } : {}),
    } satisfies SavedViewFilters,
    count,
    setters: {
      setModel: (next: string) => update({ model: next }),
      setKey: (next: string) => update({ key: next }),
      setUnits: (next: string[]) => update({ business_unit: next.join(",") }),
      setCustomers: (next: string[]) => update({ customer: next.join(",") }),
    } satisfies RowFilterSetters,
    /**
     * A saved view's own window and filters replace what was picked, and a
     * filter it does not name is cleared. One write for all of them: two
     * functional updates in a row would each start from the same address, and
     * the second would undo the first.
     */
    apply: (view: SavedViewFilters) => {
      update({
        ...windowAddress(windowFromFilters(view)),
        model: view.model ?? "",
        key: view.key ?? "",
        business_unit: (view.business_unit ?? []).join(","),
        customer: (view.customer ?? []).join(","),
      });
    },
    clear: () => update({ model: "", key: "", business_unit: "", customer: "" }),
  };
}
