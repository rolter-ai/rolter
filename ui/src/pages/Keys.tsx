import { useQuery } from "@tanstack/react-query";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tag } from "@/components/ui/tag";
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
            <CardContent className="space-y-2 text-sm text-muted-foreground">
              <div className="font-mono">{mask(key.key)}</div>
              <div className="flex flex-wrap gap-1.5">
                {key.models.length ? (
                  key.models.map((model) => <Tag key={model}>{model}</Tag>)
                ) : (
                  <Badge tone="neutral">all models</Badge>
                )}
              </div>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  );
}
