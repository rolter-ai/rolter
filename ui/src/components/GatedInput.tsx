import * as React from "react";

import { Input, type InputProps } from "@/components/ui/input";
import { RefusalWrap } from "@/components/ui/refusal-wrap";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";

/** An update field refused with the same reason and telemetry as the other gated controls. */
export function GatedInput({
  gate,
  control = "input",
  disabled,
  readOnly,
  title,
  ...props
}: InputProps & { gate: Capability; control: string }) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  const generated = React.useId();
  const id = props.id ?? generated;
  return (
    <RefusalWrap denied={denied} reason={reason} controlId={id} {...refusal}>
      <Input
        {...props}
        id={id}
        disabled={disabled || denied}
        readOnly={readOnly || denied}
        title={denied ? reason : title}
      />
    </RefusalWrap>
  );
}
