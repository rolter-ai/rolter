import { useQuery } from "@tanstack/react-query";

import { PageBody } from "@/components/screen";
import { fetchMemberships } from "@/lib/api";
import { RBAC_RESOURCES, RBAC_ROLES } from "@/lib/mock";
import { useScope } from "@/lib/scope";

const OPS: { letter: string; op: string; title: string }[] = [
  { letter: "V", op: "v", title: "View" },
  { letter: "C", op: "c", title: "Create" },
  { letter: "U", op: "u", title: "Update" },
  { letter: "D", op: "d", title: "Delete" },
];

// roles & permissions matrix from the design prototype. the capability map is
// the static contract the control API enforces (roles are fixed); member
// counts come from the live users list.
export default function Rbac() {
  const scope = useScope();
  const memberships = useQuery({
    queryKey: ["memberships", scope.orgId],
    queryFn: () => fetchMemberships(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });

  const memberCount = (role: string) =>
    new Set(
      (memberships.data ?? []).filter((m) => m.role === role).map((m) => m.user_id),
    ).size;

  return (
    <PageBody>
      <div className="flex items-start gap-3">
        <span className="ml-auto inline-flex items-center gap-1.5 rounded-full border border-[color:var(--border-subtle)] px-2.5 py-[5px] text-xs text-muted-foreground">
          {RBAC_ROLES.length} roles · {RBAC_RESOURCES.length} resources
        </span>
      </div>

      <div className="overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]">
        <div
          className="grid items-end gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[11px]"
          style={{ gridTemplateColumns: `1.5fr repeat(${RBAC_ROLES.length}, 1fr)` }}
        >
          <span className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
            Resource
          </span>
          {RBAC_ROLES.map((r) => (
            <div key={r.key} className="flex flex-col gap-0.5">
              <span className="text-sm font-semibold capitalize">{r.label}</span>
              <span className="text-[10px] text-[color:var(--text-subtle)]">
                {memberships.isError ? "—" : memberCount(r.key)} members
              </span>
            </div>
          ))}
        </div>
        {RBAC_RESOURCES.map((res) => (
          <div
            key={res.key}
            className="grid items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[11px] last:border-b-0"
            style={{ gridTemplateColumns: `1.5fr repeat(${RBAC_ROLES.length}, 1fr)` }}
          >
            <span className="text-sm">{res.label}</span>
            {RBAC_ROLES.map((r) => {
              const caps = r.caps[res.key] ?? "";
              return (
                <div key={r.key} className="flex flex-wrap gap-1">
                  {OPS.map((o) => {
                    const on = caps.includes(o.op);
                    return (
                      <span
                        key={o.op}
                        title={`${o.title} — ${on ? "allowed" : "denied"}`}
                        className="flex h-5 w-[22px] items-center justify-center rounded-[6px] border font-mono text-[10px] font-semibold"
                        style={
                          on
                            ? {
                                color: "var(--red-folk)",
                                background: "var(--red-tint)",
                                borderColor: "color-mix(in srgb, var(--red-folk) 30%, transparent)",
                              }
                            : {
                                color: "var(--text-subtle)",
                                background: "transparent",
                                borderColor: "var(--border-subtle)",
                                opacity: 0.45,
                              }
                        }
                      >
                        {o.letter}
                      </span>
                    );
                  })}
                </div>
              );
            })}
          </div>
        ))}
      </div>

      <div className="flex items-center gap-3.5 text-xs text-[color:var(--text-subtle)]">
        {OPS.map((o) => (
          <span key={o.op} className="inline-flex items-center gap-[5px]">
            <span className="flex h-[18px] w-[18px] items-center justify-center rounded-[6px] bg-[color:var(--red-tint)] font-mono text-[10px] font-semibold text-[color:var(--red-folk)]">
              {o.letter}
            </span>
            {o.title}
          </span>
        ))}
      </div>
    </PageBody>
  );
}
