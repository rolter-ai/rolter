import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, ChevronDown } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { fetchOrgs, type ConfigProblems, type OmittedKind } from "@/lib/api";
import { cn } from "@/lib/utils";

/** One line of the notice: a kind of problem, and what it is about. */
interface ProblemGroup {
  key: string;
  label: string;
  items: string[];
}

/**
 * A server line as a person reads it: no `warning:` lead and no issue number,
 * which say something to rolter's maintainers and nothing to an operator.
 */
function readable(line: string): string {
  return line
    .replace(/^warning:\s*/i, "")
    .replace(/\s*\(#\d+\)/g, "")
    .trim();
}

const SUBJECT_KINDS = new Set(["provider", "route", "provider group"]);
const OMITTED_KINDS: OmittedKind[] = ["provider", "route", "virtual_key", "public_example_key"];

/**
 * Lists config entries the control plane is not serving to gateways (#926).
 *
 * A malformed provider is dropped from the snapshot rather than freezing the
 * whole fleet's config. That is the right blast radius, but it makes the
 * failure quiet: every gateway keeps serving its last good config, so nothing
 * looks broken until someone wonders why a change never took effect. This is
 * the thing that says so.
 *
 * It says it one line per kind (#3646): thirteen English sentences filled the
 * first screen of a fresh install, with an org as a UUID and an issue number
 * in each. The structured `findings` group the lines, an org is named, and the
 * rows a line is about open on demand. A control plane older than `findings`
 * gets its sentences listed under one line, as before but folded.
 */
export function UnservedConfigNotice({ report }: { report: ConfigProblems | undefined }) {
  const { t } = useTranslation();
  const findings = report?.findings;
  const wantsOrgs = !!findings?.some((f) => f.org_id);
  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs, enabled: wantsOrgs });
  const orgName = (id: string) =>
    (Array.isArray(orgs.data) ? orgs.data : []).find((o) => o.id === id)?.name ?? id.slice(0, 8);
  const [open, setOpen] = React.useState<Set<string>>(() => new Set());

  const count = report?.problems.length ?? 0;
  if (!report || count === 0) return null;

  const groups: ProblemGroup[] = [];
  if (findings) {
    // org rows that shadow a config-file row of the same name, one line per org
    const shadows = new Map<string, string[]>();
    for (const f of findings) {
      if (f.kind !== "bootstrap_shadow" || !f.subject) continue;
      const kind = SUBJECT_KINDS.has(f.subject.kind)
        ? t(`providers.unserved.subjects.${f.subject.kind.replace(" ", "_")}`)
        : f.subject.kind;
      const list = shadows.get(f.org_id ?? "") ?? [];
      list.push(`${kind} ${f.subject.name}`);
      shadows.set(f.org_id ?? "", list);
    }
    for (const [org, items] of shadows) {
      groups.push({
        key: `shadow:${org}`,
        label: t("providers.unserved.shadowed", { count: items.length, org: orgName(org) }),
        items,
      });
    }
    // entries the snapshot leaves out whole, one line per kind
    for (const kind of OMITTED_KINDS) {
      const items = report.omitted.filter((e) => e.kind === kind).map((e) => e.name);
      if (items.length) {
        groups.push({
          key: `omitted:${kind}`,
          label: t(`providers.unserved.omitted.${kind}`, { count: items.length }),
          items,
        });
      }
    }
    // everything else in the server's own words, which nothing here can
    // translate: the line says which setting and why
    const rest = findings
      .filter((f) => f.kind !== "omitted" && !(f.kind === "bootstrap_shadow" && f.subject))
      .map((f) => readable(f.message));
    if (rest.length) {
      groups.push({
        key: "other",
        label: t("providers.unserved.other", { count: rest.length }),
        items: rest,
      });
    }
  } else {
    groups.push({
      key: "all",
      label: t("providers.unserved.other", { count }),
      items: report.problems.map(readable),
    });
  }

  const toggle = (key: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  return (
    <div
      data-testid="unserved-config"
      className="space-y-2 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--red-tint)] p-4"
    >
      <div className="flex items-center gap-2">
        <AlertTriangle
          aria-hidden
          className="h-4 w-4 flex-none text-[color:var(--status-warning-text)]"
        />
        <p className="text-sm font-medium text-foreground">
          {t("providers.unserved.title", { count })}
        </p>
      </div>
      <ul className="space-y-1 pl-6">
        {groups.map((group) => {
          const expanded = open.has(group.key);
          const listId = `unserved-${group.key.replace(/[^a-z0-9]+/gi, "-")}`;
          return (
            <li key={group.key} className="text-sm text-muted-foreground">
              <button
                type="button"
                aria-expanded={expanded}
                aria-controls={listId}
                onClick={() => toggle(group.key)}
                className="inline-flex min-h-6 items-center gap-1 rounded-sm text-left hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <ChevronDown
                  aria-hidden
                  className={cn(
                    "h-3.5 w-3.5 flex-none text-[color:var(--status-warning-text)] transition-transform",
                    !expanded && "-rotate-90",
                  )}
                />
                {group.label}
              </button>
              {expanded && (
                <ul id={listId} className="mt-1 space-y-0.5 pl-5">
                  {group.items.map((item, i) => (
                    <li key={`${i}-${item}`} className="list-disc break-words">
                      {item}
                    </li>
                  ))}
                </ul>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
