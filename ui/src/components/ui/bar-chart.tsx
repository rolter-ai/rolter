import { useFormat } from "@/lib/i18n/format";
import { useMeasuredWidth } from "@/lib/use-measured-width";
import { cn } from "@/lib/utils";

// vertical bars (doubles as a histogram) with an optional horizontal ranked
// mode for 10+ categories
export interface BarChartProps extends React.HTMLAttributes<HTMLDivElement> {
  data: number[];
  labels?: (string | number)[];
  height?: number;
  color?: string;
  highlightColor?: string;
  highlightLast?: boolean;
  horizontal?: boolean;
  topN?: number;
  unit?: string;
  yMax?: number;
  gap?: number;
  /**
   * Accessible name for the graphic. `role="img"` promises a name, and axe
   * fails the story when there is none (#1181); a chart the caller does not
   * name is treated as decorative instead, because the heading and the figures
   * beside it already carry the fact. Pass a translated string.
   */
  label?: string;
}

export function BarChart({
  data = [],
  labels = [],
  height = 180,
  color = "var(--zinc-500)",
  highlightColor = "var(--red-folk)",
  highlightLast = false,
  horizontal = false,
  topN,
  unit = "",
  yMax,
  gap = 0.25,
  label,
  className,
  ...props
}: BarChartProps) {
  // axis labels are read by a person, so they follow the dashboard locale: the
  // hand-rolled `1.2k` printed a dot as the decimal mark in every language
  const fmt = useFormat();
  // drawn at the width it is given, so its 11 unit labels are 11px at every width (#3450)
  const { ref: frame, width: W } = useMeasuredWidth<HTMLDivElement>(!horizontal);
  if (horizontal) {
    return (
      <RankedBars
        data={data}
        labels={labels}
        color={color}
        highlightColor={highlightColor}
        topN={topN}
        unit={unit}
        max={yMax}
        label={label}
        className={className}
        {...props}
      />
    );
  }
  const padL = 44;
  const padR = 12;
  const padT = 12;
  const padB = 26;
  const H = height;
  const iw = W - padL - padR;
  const ih = H - padT - padB;
  const max = yMax ?? (Math.max(...data, 0) * 1.15 || 1);
  const n = data.length || 1;
  const band = iw / n;
  const bw = band * (1 - gap);
  const yAt = (v: number) => padT + ih - (v / max) * ih;
  const ticks = 4;

  return (
    <div ref={frame} className={className} {...props}>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        width="100%"
        height={H}
        {...(label ? { role: "img", "aria-label": label } : { "aria-hidden": true })}
        preserveAspectRatio="xMidYMid meet"
        style={{ display: "block", overflow: "visible" }}
      >
        {Array.from({ length: ticks + 1 }).map((_, i) => {
          const v = (max / ticks) * (ticks - i);
          const y = padT + (ih / ticks) * i;
          return (
            <g key={i}>
              <line
                x1={padL}
                y1={y}
                x2={W - padR}
                y2={y}
                stroke="var(--border-subtle)"
                strokeWidth="1"
              />
              <text
                x={padL - 8}
                y={y + 4}
                textAnchor="end"
                fontSize="11"
                fontFamily="var(--font-mono)"
                fill="var(--text-subtle)"
              >
                {fmt.compact(v)}
                {unit}
              </text>
            </g>
          );
        })}
        {data.map((v, i) => {
          const x = padL + band * i + (band - bw) / 2;
          const y = yAt(v);
          const h = padT + ih - y;
          const fill = highlightLast && i === data.length - 1 ? highlightColor : color;
          return <rect key={i} x={x} y={y} width={bw} height={Math.max(h, 0)} rx="2" fill={fill} />;
        })}
        {labels.map((l, i) => (
          <text
            key={i}
            x={padL + band * i + band / 2}
            y={H - 8}
            textAnchor="middle"
            fontSize="11"
            fontFamily="var(--font-mono)"
            fill="var(--text-subtle)"
          >
            {l}
          </text>
        ))}
      </svg>
    </div>
  );
}

interface RankedBarsProps extends React.HTMLAttributes<HTMLDivElement> {
  data: number[];
  labels: (string | number)[];
  color: string;
  highlightColor: string;
  topN?: number;
  unit: string;
  max?: number;
  label?: string;
}

// horizontal ranked bars — the readable pattern for 10+ categories.
function RankedBars({
  data,
  labels,
  color,
  highlightColor,
  topN,
  unit,
  max,
  label,
  className,
  ...props
}: RankedBarsProps) {
  const fmt = useFormat();
  let rows = data.map((v, i) => ({ v, label: labels[i] != null ? labels[i] : String(i) }));
  rows.sort((a, b) => b.v - a.v);
  if (topN) rows = rows.slice(0, topN);
  const mx = max ?? (Math.max(...rows.map((r) => r.v), 0) || 1);
  return (
    <div
      className={className}
      style={{ display: "flex", flexDirection: "column", gap: 6 }}
      // rows of real text rather than an <svg>, so this needs no role to be
      // read — a name is only attached when the caller supplies one, and
      // `aria-label` on a bare <div> is a prohibited attribute (#1181)
      {...(label ? { role: "group", "aria-label": label } : {})}
      {...props}
    >
      {rows.map((r, i) => (
        <div key={i} style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span
            style={{
              width: 96,
              flex: "none",
              textAlign: "right",
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-xs)",
              color: "var(--text-muted)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {r.label}
          </span>
          <BarTrack
            share={r.v / mx}
            color={i === 0 ? highlightColor : color}
            style={{ flex: 1, height: 16 }}
          />
          <span
            style={{
              width: 56,
              flex: "none",
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-xs)",
              color: "var(--text-secondary)",
            }}
          >
            {fmt.compact(r.v)}
            {unit}
          </span>
        </div>
      ))}
    </div>
  );
}

/**
 * One horizontal bar: a quiet track with a fill of `share` (0 to 1) of its
 * width. The ranked chart draws each of its rows with it, and a table cell that
 * wants a bar beside its number uses it too, sized by `className` or `style`
 * (the dashboard's by-model table), so there is one bar and not one per screen.
 * Decorative: the number beside it is what a reader takes in.
 */
export function BarTrack({
  share,
  color = "var(--zinc-500)",
  className,
  style,
}: {
  share: number;
  color?: string;
  className?: string;
  style?: React.CSSProperties;
}) {
  return (
    <span
      aria-hidden
      className={cn("block", className)}
      style={{
        background: "var(--surface-subtle)",
        borderRadius: 3,
        overflow: "hidden",
        ...style,
      }}
    >
      <span
        style={{
          display: "block",
          width: `${Math.min(1, Math.max(0, share)) * 100}%`,
          height: "100%",
          background: color,
          borderRadius: 3,
        }}
      />
    </span>
  );
}
