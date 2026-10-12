import { useTranslation } from "react-i18next";

import { StrategyHint } from "@/components/StrategyHint";
import { Combobox } from "@/components/ui/combobox";
import { strategyDescriptionKey, strategyGroup, strategyOptions } from "@/lib/strategies";

/**
 * The balancing-strategy dropdown, the same in the model sheet, the route sheet
 * and the provider group sheet (#2136).
 *
 * Fourteen bare identifiers are past what anyone holds in mind at one decision
 * point, so each option carries its one-line description and sits under its
 * group, and the selected strategy's description stays under the control with
 * its caveat, if it has one. The names stay verbatim (PRODUCT.md): only the
 * description and the group heading are translated.
 */
export function StrategyPicker({
  id,
  value,
  onChange,
  disabled,
}: {
  id: string;
  value: string;
  onChange: (strategy: string) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const options = strategyOptions(value).map((strategy) => {
    const group = strategyGroup(strategy);
    const description = strategyDescriptionKey(strategy);
    return {
      value: strategy,
      label: strategy,
      group: group ? t(`strategies.groups.${group}`) : undefined,
      description: description ? t(description) : undefined,
    };
  });
  const selected = strategyDescriptionKey(value);
  const describedBy = `${id}-description`;
  return (
    <>
      <Combobox
        id={id}
        className="font-mono"
        value={value}
        disabled={disabled}
        onChange={onChange}
        options={options}
        aria-describedby={selected ? describedBy : undefined}
      />
      {selected && (
        <p id={describedBy} className="mt-1.5 text-xs text-muted-foreground">
          {t(selected)}
        </p>
      )}
      <StrategyHint strategy={value} />
    </>
  );
}
