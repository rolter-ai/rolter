import { useQuery } from "@tanstack/react-query";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { fetchConfig } from "@/lib/api";

function mask(key: string): string {
  if (key.length <= 8) {
    return "••••";
  }
  return `${key.slice(0, 6)}…${key.slice(-2)}`;
}

export default function Keys() {
  const { data } = useQuery({ queryKey: ["config"], queryFn: fetchConfig });

  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Virtual keys</h1>
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {data?.virtual_keys.map((key) => (
          <Card key={key.key}>
            <CardHeader>
              <CardTitle>{key.name ?? "key"}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-1 text-sm text-muted-foreground">
              <div className="font-mono">{mask(key.key)}</div>
              <div>{key.models.length ? key.models.join(", ") : "all models"}</div>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  );
}
