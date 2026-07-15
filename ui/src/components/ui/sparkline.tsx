// tiny inline trend for tables and stat tiles — no axes.
// ported from the Rolter Design System components/charts/Sparkline.jsx
export interface SparklineProps extends Omit<React.SVGProps<SVGSVGElement>, "values"> {
  values: number[];
  width?: number;
  height?: number;
  color?: string;
  area?: boolean;
}

export function Sparkline({
  values = [],
  width = 100,
  height = 28,
  color = "var(--zinc-300)",
  area = true,
  className,
  ...props
}: SparklineProps) {
  const padY = 3;
  const max = Math.max(...values, 0);
  const min = Math.min(...values, 0);
  const range = max - min || 1;
  const n = values.length;
  const xAt = (i: number) => (n <= 1 ? width / 2 : (i / (n - 1)) * width);
  const yAt = (v: number) =>
    padY + (height - padY * 2) - ((v - min) / range) * (height - padY * 2);
  const line = values.map((v, i) => `${xAt(i).toFixed(1)},${yAt(v).toFixed(1)}`).join(" ");
  const fill = `M0,${height} L ${line.split(" ").join(" L ")} L ${width},${height} Z`;
  return (
    <svg
      className={className}
      viewBox={`0 0 ${width} ${height}`}
      width={width}
      height={height}
      preserveAspectRatio="none"
      style={{ display: "block", overflow: "visible" }}
      {...props}
    >
      {area && <path d={fill} fill={color} opacity="0.14" />}
      <polyline
        points={line}
        fill="none"
        stroke={color}
        strokeWidth="1.5"
        strokeLinejoin="round"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
      {n > 0 && <circle cx={xAt(n - 1)} cy={yAt(values[n - 1])} r="2" fill={color} />}
    </svg>
  );
}
