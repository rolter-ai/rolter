import { useQuery } from "@tanstack/react-query";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { fetchConfig } from "@/lib/api";

export default function Models() {
  const { data, isLoading, error } = useQuery({
    queryKey: ["config"],
    queryFn: fetchConfig,
  });

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold">Models</h1>
        <p className="text-sm text-muted-foreground">
          Public model names routed by rolter.
        </p>
      </div>
      {isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {error && <p className="text-sm text-destructive">Failed to load config.</p>}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {data?.routes.map((route) => (
          <Card key={route.model}>
            <CardHeader>
              <CardTitle>{route.model}</CardTitle>
              <CardDescription>
                {route.strategy} · {route.targets.length} target(s)
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-1 text-sm text-muted-foreground">
              {route.targets.map((target, i) => (
                <div key={i}>
                  {target.provider}
                  {target.model ? ` → ${target.model}` : ""}
                </div>
              ))}
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  );
}
