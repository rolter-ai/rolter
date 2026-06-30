import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

export default function Logs() {
  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Logs &amp; cost</h1>
      <Card>
        <CardHeader>
          <CardTitle>Request logs</CardTitle>
          <CardDescription>
            Per-request usage and cost streamed from ClickHouse.
          </CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          Not wired yet — tracked in TODO.md (logging + cost tracking phase).
        </CardContent>
      </Card>
    </div>
  );
}
