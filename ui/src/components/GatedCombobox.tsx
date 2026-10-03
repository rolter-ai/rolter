import * as React from "react";

import { Combobox, type ComboboxProps } from "@/components/ui/combobox";
import { RefusalWrap } from "@/components/ui/refusal-wrap";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";

/**
 * The `Combobox` a row setting uses, refused the way a switch is (#1759).
 *
 * A picker on a row — a key's cache mode — is the same `<resource>:update` its
 * toggle and its edit sheet need. Keys used to disable it from its own
 * `useGate`, which disabled it silently: the reach for it never reached the UX
 * stream. `control` names it there, and is required for the reason it is on
 * `GatedButton` (#1750).
 */
export function GatedCombobox({
  gate,
  control = "combobox",
  disabled,
  title,
  ...props
}: ComboboxProps & { gate: Capability; control: string }) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  const generated = React.useId();
  const id = props.id ?? generated;
  return (
    <RefusalWrap denied={denied} reason={reason} controlId={id} {...refusal}>
      <Combobox {...props} id={id} disabled={disabled || denied} title={denied ? reason : title} />
    </RefusalWrap>
  );
}
