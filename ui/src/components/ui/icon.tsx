import * as React from "react";

import { cn } from "@/lib/utils";

// sizing + color wrapper around an SVG (the product's Lucide icon set).
// pass a raw <svg> as children, or an inline path via `path`. normalises size,
// stroke and color to currentColor. mirrors the Rolter Design System
// foundation/Icon.
export interface IconProps extends Omit<React.SVGProps<SVGSVGElement>, "path"> {
  size?: number;
  strokeWidth?: number;
  path?: string | string[];
}

export function Icon({
  size = 16,
  strokeWidth = 2,
  path,
  className,
  style,
  children,
  ...props
}: IconProps) {
  const common: React.SVGProps<SVGSVGElement> = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    "aria-hidden": true,
    className: cn("inline-block flex-none", className),
    style,
    ...props,
  };
  if (path) {
    return (
      <svg {...common}>
        {Array.isArray(path) ? path.map((d, i) => <path key={i} d={d} />) : <path d={path} />}
      </svg>
    );
  }
  // wrap a provided <svg> child: clone with normalised sizing.
  if (React.isValidElement(children) && children.type === "svg") {
    const child = children as React.ReactElement<React.SVGProps<SVGSVGElement>>;
    return React.cloneElement(child, {
      ...common,
      viewBox: child.props.viewBox || common.viewBox,
    });
  }
  return <svg {...common}>{children}</svg>;
}
