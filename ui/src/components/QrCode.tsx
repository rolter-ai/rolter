import qrcode from "qrcode-generator";
import * as React from "react";

import { cn } from "@/lib/utils";

// QR renderer for the enrolment secret (#1078).
//
// Drawn as an inline SVG rather than fetched from a chart service, because a
// second-factor secret sent to a third party is not a second factor, and
// because the dashboard has to work air-gapped — the encoder is bundled, so
// nothing here reaches the network.
//
// The image is `aria-hidden` on purpose: a QR is unreadable to a screen
// reader, and the same secret sits beside it as selectable base32 text, which
// is the accessible path and the one a user without a camera needs anyway.
export interface QrCodeProps {
  /** the payload; an `otpauth://` URI here */
  value: string;
  /** rendered edge length in px */
  size?: number;
  className?: string;
}

/**
 * Quiet zone, in modules. Four is the spec minimum, and scanners really do
 * fail without it when the code sits on a bordered card like this one.
 */
const QUIET_ZONE = 4;

export function QrCode({ value, size = 176, className }: QrCodeProps) {
  const path = React.useMemo(() => {
    // `0` picks the smallest version that fits; `M` is the level every
    // authenticator app's own docs assume
    const qr = qrcode(0, "M");
    qr.addData(value);
    qr.make();
    const count = qr.getModuleCount();
    const parts: string[] = [];
    for (let row = 0; row < count; row += 1) {
      for (let col = 0; col < count; col += 1) {
        // one path over all dark modules rather than a rect each: a version-4
        // code is ~700 elements, and the browser lays out every one of them
        if (qr.isDark(row, col)) {
          parts.push(`M${col + QUIET_ZONE} ${row + QUIET_ZONE}h1v1h-1z`);
        }
      }
    }
    return { d: parts.join(""), extent: count + QUIET_ZONE * 2 };
  }, [value]);

  return (
    <svg
      aria-hidden
      focusable="false"
      width={size}
      height={size}
      viewBox={`0 0 ${path.extent} ${path.extent}`}
      className={cn("rounded-md bg-white p-0", className)}
      shapeRendering="crispEdges"
    >
      {/* white behind the modules whatever the surface underneath is: a QR
          inverted by a dark theme does not scan */}
      <rect width={path.extent} height={path.extent} fill="#ffffff" />
      <path d={path.d} fill="#000000" />
    </svg>
  );
}
