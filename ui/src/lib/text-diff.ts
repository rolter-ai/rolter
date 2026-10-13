// a small line diff for comparing two versions of a text (#2112).
//
// the versions an operator compares are a SKILL.md or a handful of decorators,
// a few hundred lines at most, so a longest-common-subsequence over lines is
// plenty and keeps a diff library, and its bundle weight, out of the dashboard.
// a pair too large for the table falls back to the shared head and tail, which
// still reports every changed line, only as one block instead of a minimal edit

export type DiffKind = "same" | "add" | "del";

export interface DiffLine {
  kind: DiffKind;
  text: string;
  /** 1-based line number in the older text; absent on an added line */
  oldNo?: number;
  /** 1-based line number in the newer text; absent on a removed line */
  newNo?: number;
}

/** the largest table (old lines x new lines) the exact diff is built for */
const TABLE_LIMIT = 4_000_000;

function split(text: string): string[] {
  if (text === "") return [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  // a trailing newline ends the last line, it does not start another
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** the lines of `after` against `before`, in reading order */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = split(before);
  const b = split(after);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  ) {
    tail++;
  }
  const midA = a.slice(head, a.length - tail);
  const midB = b.slice(head, b.length - tail);

  const out: DiffLine[] = [];
  let oldNo = 1;
  let newNo = 1;
  const push = (kind: DiffKind, text: string) => {
    if (kind === "same") out.push({ kind, text, oldNo: oldNo++, newNo: newNo++ });
    else if (kind === "del") out.push({ kind, text, oldNo: oldNo++ });
    else out.push({ kind, text, newNo: newNo++ });
  };

  for (let i = 0; i < head; i++) push("same", a[i]);
  for (const step of middle(midA, midB)) push(step.kind, step.text);
  for (let i = a.length - tail; i < a.length; i++) push("same", a[i]);
  return out;
}

function middle(a: string[], b: string[]): { kind: DiffKind; text: string }[] {
  if (a.length === 0) return b.map((text) => ({ kind: "add", text }));
  if (b.length === 0) return a.map((text) => ({ kind: "del", text }));
  if (a.length * b.length > TABLE_LIMIT) {
    return [
      ...a.map((text) => ({ kind: "del" as const, text })),
      ...b.map((text) => ({ kind: "add" as const, text })),
    ];
  }
  // lcs[i][j]: the longest common run of a[i..] and b[j..]
  const width = b.length + 1;
  const lcs = new Uint32Array((a.length + 1) * width);
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i * width + j] =
        a[i] === b[j]
          ? lcs[(i + 1) * width + j + 1] + 1
          : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }
  const out: { kind: DiffKind; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: "same", text: a[i] });
      i++;
      j++;
    } else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
      out.push({ kind: "del", text: a[i++] });
    } else {
      out.push({ kind: "add", text: b[j++] });
    }
  }
  while (i < a.length) out.push({ kind: "del", text: a[i++] });
  while (j < b.length) out.push({ kind: "add", text: b[j++] });
  return out;
}

/** how many lines the newer text added and removed */
export function countChanges(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "add") added++;
    else if (line.kind === "del") removed++;
  }
  return { added, removed };
}

/** a run of unchanged lines folded away, standing between two changes */
export interface DiffSkip {
  kind: "skip";
  count: number;
}

/**
 * The diff with every run of unchanged lines longer than `2 * context` folded
 * to its `context` lines beside a change and a skip marker between them.
 */
export function foldUnchanged(lines: DiffLine[], context = 3): (DiffLine | DiffSkip)[] {
  const near = new Array<boolean>(lines.length).fill(false);
  lines.forEach((line, index) => {
    if (line.kind === "same") return;
    for (
      let k = Math.max(0, index - context);
      k <= Math.min(lines.length - 1, index + context);
      k++
    )
      near[k] = true;
  });
  const out: (DiffLine | DiffSkip)[] = [];
  let skipped = 0;
  lines.forEach((line, index) => {
    if (near[index]) {
      if (skipped) out.push({ kind: "skip", count: skipped });
      skipped = 0;
      out.push(line);
    } else {
      skipped++;
    }
  });
  if (skipped) out.push({ kind: "skip", count: skipped });
  return out;
}

export interface SplitRow {
  left?: DiffLine;
  right?: DiffLine;
}

/** the diff as side-by-side rows: a run of removals pairs with the additions that follow it */
export function pairLines(lines: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.kind === "same") {
      rows.push({ left: line, right: line });
      i++;
      continue;
    }
    const removed: DiffLine[] = [];
    const added: DiffLine[] = [];
    while (i < lines.length && lines[i].kind === "del") removed.push(lines[i++]);
    while (i < lines.length && lines[i].kind === "add") added.push(lines[i++]);
    for (let k = 0; k < Math.max(removed.length, added.length); k++) {
      rows.push({ left: removed[k], right: added[k] });
    }
  }
  return rows;
}

export type FieldChangeKind = "added" | "removed" | "changed" | "same";

export interface FieldChange {
  field: string;
  kind: FieldChangeKind;
  before?: string;
  after?: string;
}

/**
 * The fields of two records side by side, in the order they first appear
 * (`before`'s, then those only `after` has). A field in both with equal text
 * is `same`.
 */
export function diffFields(
  before: Record<string, string>,
  after: Record<string, string>,
): FieldChange[] {
  const names = [...Object.keys(before), ...Object.keys(after).filter((key) => !(key in before))];
  return names.map((field) => {
    const had = field in before;
    const has = field in after;
    if (had && !has) return { field, kind: "removed", before: before[field] };
    if (!had && has) return { field, kind: "added", after: after[field] };
    return before[field] === after[field]
      ? { field, kind: "same", before: before[field], after: after[field] }
      : { field, kind: "changed", before: before[field], after: after[field] };
  });
}
