import { ChevronDown, ChevronRight } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ChipGroup } from "@/components/ui/chip-group";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import {
  FAILURE_CLASSES,
  isRestricted,
  REFUSAL_PREFIX,
  refusalCategoryValid,
  toggled,
} from "@/lib/failure-classes";

/**
 * The mark a restricted fallback carries wherever a target is listed: it takes
 * no ordinary traffic, so it is named a fallback rather than given a share.
 * The title says which failures it takes over.
 */
export function FallbackBadge({ takesOver }: { takesOver: readonly string[] }) {
  const { t } = useTranslation();
  if (!isRestricted(takesOver)) return null;
  return (
    <Badge
      tone="info"
      data-testid="fallback-badge"
      title={t("fallbackTargets.badgeTitle", { classes: takesOver.join(", ") })}
    >
      {t("fallbackTargets.badge")}
    </Badge>
  );
}

export interface TakesOverFieldProps {
  /** the target's set, as the API stores it: class tokens, in the operator's order */
  value: string[];
  onChange: (next: string[]) => void;
  /** names the target's set for a screen reader, `Fallback classes of target 2`; translated */
  groupLabel: string;
  /** a refusal or a problem the sheet found with this set, already translated */
  error?: string;
  disabled?: boolean;
}

/**
 * A target's `takes_over` (#1584, #3319): the failures it is a fallback for.
 *
 * Folded under one line that says what the target is, an ordinary target or a
 * fallback for named failures, since most targets are ordinary and a chip group
 * under every line would bury the list. A target that already takes something
 * over opens unfolded. The six classes are chips; a refusal narrowed to one
 * category (`refusal:<category>`) is typed in and then shows as a chip of its
 * own.
 */
export function TakesOverField({
  value,
  onChange,
  groupLabel,
  error,
  disabled,
}: TakesOverFieldProps) {
  const { t } = useTranslation();
  const restricted = isRestricted(value);
  const [open, setOpen] = React.useState(restricted || !!error);
  React.useEffect(() => {
    if (error) setOpen(true);
  }, [error]);
  const [category, setCategory] = React.useState("");
  const [categoryError, setCategoryError] = React.useState("");
  const fid = React.useId();
  const ids = {
    panel: `${fid}-panel`,
    category: `${fid}-category`,
    categoryErr: `${fid}-category-err`,
    err: `${fid}-err`,
  };

  const categories = value.filter((token) => token.startsWith(REFUSAL_PREFIX));
  const options = [...FAILURE_CLASSES, ...categories].map((token) => ({
    id: token,
    name: token,
  }));
  const addCategory = () => {
    const trimmed = category.trim();
    if (!refusalCategoryValid(trimmed)) {
      setCategoryError(t("fallbackTargets.errors.category"));
      return;
    }
    const token = `${REFUSAL_PREFIX}${trimmed}`;
    if (!value.includes(token)) onChange([...value, token]);
    setCategory("");
    setCategoryError("");
  };

  return (
    <div className="col-span-full space-y-2" role="group" aria-label={groupLabel}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 px-1.5 text-xs text-muted-foreground"
          aria-expanded={open}
          aria-controls={ids.panel}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy(error && ids.err)}
          data-error-anchor={error ? true : undefined}
          onClick={() => setOpen((o) => !o)}
        >
          {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          {restricted
            ? t("fallbackTargets.summaryRestricted", { classes: value.join(", ") })
            : t("fallbackTargets.summaryOrdinary")}
        </Button>
        <FallbackBadge takesOver={value} />
      </div>
      {open && (
        <div id={ids.panel} className="space-y-2 pl-6">
          <ChipGroup
            label={t("fallbackTargets.label")}
            options={options}
            selected={value}
            disabled={disabled}
            onToggle={(token) => onChange(toggled(value, token))}
          />
          <div className="space-y-1.5">
            <FieldLabel
              label={t("fallbackTargets.category")}
              info={t("fallbackTargets.categoryInfo")}
              htmlFor={ids.category}
            />
            <div className="flex max-w-sm items-center gap-2">
              <Input
                id={ids.category}
                className="min-w-0 flex-1 font-mono"
                value={category}
                disabled={disabled}
                placeholder={t("fallbackTargets.categoryPlaceholder")}
                aria-invalid={categoryError ? true : undefined}
                aria-describedby={describedBy(categoryError && ids.categoryErr)}
                onChange={(e) => {
                  setCategory(e.target.value);
                  setCategoryError("");
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addCategory();
                  }
                }}
              />
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled || category.trim() === ""}
                onClick={addCategory}
              >
                {t("fallbackTargets.addCategory")}
              </Button>
            </div>
            <FieldError id={ids.categoryErr} error={categoryError} />
          </div>
          <p className="text-xs leading-snug text-muted-foreground">{t("fallbackTargets.hint")}</p>
        </div>
      )}
      <FieldError id={ids.err} error={error} />
    </div>
  );
}
