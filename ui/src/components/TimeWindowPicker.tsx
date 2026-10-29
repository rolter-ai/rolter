import * as React from "react";
import { useTranslation } from "react-i18next";

import { Combobox } from "@/components/ui/combobox";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { browserTimeZone, chartTimeZone, useFormat } from "@/lib/i18n/format";
import {
  CUSTOM_WINDOW,
  customRange,
  customWindow,
  dayRangeProblem,
  formatDay,
  readTimeWindow,
  useWindowSelectionOptions,
  windowBounds,
  type DayRange,
  type WindowSelection,
} from "@/lib/time-window";
import { cn } from "@/lib/utils";

/**
 * The window picker every reporting screen shares (#3407): the named windows,
 * and a custom range of two days.
 *
 * Choosing "Custom range" opens a start and an end day under the list, seeded
 * with the days the current window covers so the screen keeps showing what it
 * was showing. A range is committed only when it is whole: an end before the
 * start is said at the field and the screen keeps the last good range, so it is
 * never asked for a window that does not exist. The days are calendar days in
 * the chart zone (#3500) and the end day is included; where that zone is not
 * the browser's, a line under the list names it, so "1 to 7 October" is never
 * read as the browser's October.
 *
 * `value` and `onChange` carry a {@link WindowSelection}; where it is kept (the
 * address, a saved view) is the screen's. `id` and the `aria-*` attributes land
 * on the list, so a `Field` around the picker labels it and describes it: the
 * hint or error `Field` ties by `aria-describedby`, and its `aria-invalid` and
 * `aria-required`, reach the control a screen reader announces (#3507).
 */
export function TimeWindowPicker({
  value,
  onChange,
  className,
  id,
  "aria-label": ariaLabel,
  "aria-describedby": ariaDescribedBy,
  "aria-invalid": ariaInvalid,
  "aria-required": ariaRequired,
}: {
  value: WindowSelection;
  onChange: (next: WindowSelection) => void;
  className?: string;
  id?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  "aria-required"?: boolean | "true" | "false";
}) {
  const { t } = useTranslation();
  // subscribes to the chart zone, so the note below follows a change to it
  useFormat();
  const options = useWindowSelectionOptions();
  const uid = React.useId();
  const zone = chartTimeZone();
  const countedIn = zone && zone !== browserTimeZone() ? zone : undefined;
  const committed = customRange(value);
  // what the two day fields hold, which may be a half-typed or refused range
  // while `value` is still the last good one
  const [draft, setDraft] = React.useState<DayRange>(committed ?? { since: "", until: "" });
  React.useEffect(() => {
    if (committed) setDraft(committed);
    // keyed on the days, not on the object, which is new on every render
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [committed?.since, committed?.until]);

  const problem = committed ? dayRangeProblem(draft) : null;
  const today = formatDay(new Date());

  const choose = (next: string) => {
    if (next !== CUSTOM_WINDOW) {
      onChange(readTimeWindow(next));
      return;
    }
    if (committed) return;
    // start from the days the window on screen covers
    const bounds = windowBounds(value);
    const seed = {
      since: formatDay(new Date(bounds.since)),
      until: formatDay(bounds.until ? new Date(Date.parse(bounds.until) - 1) : new Date()),
    };
    const range = customWindow(seed.since, seed.until);
    if (range) onChange(range);
  };

  const edit = (patch: Partial<DayRange>) => {
    const next = { ...draft, ...patch };
    setDraft(next);
    const range = customWindow(next.since, next.until);
    if (range) onChange(range);
  };

  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <Combobox
        id={id}
        aria-label={ariaLabel}
        aria-describedby={describedBy(ariaDescribedBy, countedIn ? `${uid}-zone` : undefined)}
        aria-invalid={ariaInvalid}
        aria-required={ariaRequired}
        options={options}
        value={committed ? CUSTOM_WINDOW : value}
        onChange={choose}
      />
      {countedIn && (
        <p id={`${uid}-zone`} className="text-xs text-muted-foreground">
          {t("common.timeWindow.countedIn", { zone: countedIn })}
        </p>
      )}
      {committed && (
        <div
          className="grid grid-cols-2 gap-2"
          role="group"
          aria-label={t("common.timeWindow.range")}
        >
          <div className="min-w-0 space-y-1">
            <FieldLabel label={t("common.timeWindow.from")} htmlFor={`${uid}-since`} />
            <Input
              id={`${uid}-since`}
              type="date"
              className="h-8 px-2 text-xs"
              value={draft.since}
              max={draft.until || today}
              aria-invalid={problem ? true : undefined}
              aria-describedby={describedBy(problem ? `${uid}-problem` : undefined)}
              onChange={(e) => edit({ since: e.target.value })}
            />
          </div>
          <div className="min-w-0 space-y-1">
            <FieldLabel label={t("common.timeWindow.to")} htmlFor={`${uid}-until`} />
            <Input
              id={`${uid}-until`}
              type="date"
              className="h-8 px-2 text-xs"
              value={draft.until}
              min={draft.since || undefined}
              max={today}
              aria-invalid={problem ? true : undefined}
              aria-describedby={describedBy(problem ? `${uid}-problem` : undefined)}
              onChange={(e) => edit({ until: e.target.value })}
            />
          </div>
          <div className="col-span-2">
            <FieldError
              id={`${uid}-problem`}
              error={
                problem === "order"
                  ? t("common.timeWindow.endBeforeStart")
                  : problem
                    ? t("common.timeWindow.pickBothDays")
                    : undefined
              }
            />
          </div>
        </div>
      )}
    </div>
  );
}
