// one place for how a dashboard export writes a CSV cell, so a name typed by
// someone other than the reader cannot shift a column or run as a formula

/** a spreadsheet runs a cell that starts with one of these as a formula */
const FORMULA_START = /^[=+\-@\t\r]/;

/** one cell: formula-safe, and quoted when it holds a comma, a quote or a line break */
export function csvCell(value: unknown): string {
  let text = value == null ? "" : typeof value === "string" ? value : JSON.stringify(value);
  if (FORMULA_START.test(text)) text = `'${text}`;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

export function csvLine(values: unknown[]): string {
  return values.map(csvCell).join(",");
}
