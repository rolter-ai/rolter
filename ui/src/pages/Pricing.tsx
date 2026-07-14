import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  deleteModelPrice,
  fetchModelPrices,
  upsertModelPrice,
  type ModelPriceRow,
} from "@/lib/api";

const PRICES_QUERY_KEY = ["model-prices"];

// global model pricing catalog — no org/team/project scoping. upsert is
// keyed on `model`, so add and edit share one dialog/mutation.
export default function Pricing() {
  const queryClient = useQueryClient();

  const prices = useQuery({
    queryKey: PRICES_QUERY_KEY,
    queryFn: fetchModelPrices,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: PRICES_QUERY_KEY });

  const removePrice = useMutation({
    mutationFn: (model: string) => deleteModelPrice(model),
    onSuccess: invalidate,
  });

  const [editOpen, setEditOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<ModelPriceRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<ModelPriceRow | null>(null);

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Pricing</h1>
          <p className="text-sm text-muted-foreground">
            Per-model token pricing catalog, used for cost accounting. Global —
            not scoped to an org/project.
          </p>
        </div>
        <Button
          size="sm"
          onClick={() => {
            setEditTarget(null);
            setEditOpen(true);
          }}
        >
          <Plus className="h-4 w-4" />
          Add price
        </Button>
      </div>

      {prices.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {prices.error && (
        <p className="text-sm text-destructive">Failed to load model prices.</p>
      )}
      {!prices.isLoading && prices.data?.length === 0 && (
        <p className="text-sm text-muted-foreground">No model prices set yet.</p>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {prices.data?.map((price) => (
          <Card key={price.id}>
            <CardHeader>
              <CardTitle className="truncate">{price.model}</CardTitle>
              <CardDescription className="flex flex-wrap items-center gap-1.5">
                <Badge tone="outline">
                  in {price.input_per_mtok} {price.currency}/Mtok
                </Badge>
                <Badge tone="outline">
                  out {price.output_per_mtok} {price.currency}/Mtok
                </Badge>
                {price.cached_input_per_mtok && (
                  <Badge tone="neutral">
                    cached {price.cached_input_per_mtok} {price.currency}/Mtok
                  </Badge>
                )}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex justify-end gap-1">
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  setEditTarget(price);
                  setEditOpen(true);
                }}
              >
                Edit
              </Button>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => setDeleteTarget(price)}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </CardContent>
          </Card>
        ))}
      </div>

      <UpsertPriceDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        existing={editTarget}
        onDone={invalidate}
      />

      <Dialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <DialogHeader>
          <DialogTitle>Delete price</DialogTitle>
          <DialogDescription>
            Removes the pricing entry for{" "}
            <span className="font-mono">{deleteTarget?.model}</span>. Cost
            accounting for this model falls back to no known price.
          </DialogDescription>
        </DialogHeader>
        {removePrice.isError && (
          <p className="text-xs text-destructive">
            {(removePrice.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removePrice.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              removePrice.mutate(deleteTarget.model, {
                onSuccess: () => setDeleteTarget(null),
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </div>
  );
}

function UpsertPriceDialog({
  open,
  onOpenChange,
  existing,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing: ModelPriceRow | null;
  onDone: () => void;
}) {
  const [model, setModel] = React.useState("");
  const [inputPerMtok, setInputPerMtok] = React.useState("0");
  const [outputPerMtok, setOutputPerMtok] = React.useState("0");
  const [cachedInputPerMtok, setCachedInputPerMtok] = React.useState("");
  const [currency, setCurrency] = React.useState("USD");

  React.useEffect(() => {
    if (open) {
      setModel(existing?.model ?? "");
      setInputPerMtok(existing?.input_per_mtok ?? "0");
      setOutputPerMtok(existing?.output_per_mtok ?? "0");
      setCachedInputPerMtok(existing?.cached_input_per_mtok ?? "");
      setCurrency(existing?.currency ?? "USD");
    }
  }, [open, existing]);

  const submit = useMutation({
    mutationFn: () =>
      upsertModelPrice({
        model,
        input_per_mtok: inputPerMtok,
        output_per_mtok: outputPerMtok,
        cached_input_per_mtok: cachedInputPerMtok.trim() || undefined,
        currency,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{existing ? `Edit ${existing.model}` : "Add price"}</DialogTitle>
        <DialogDescription>
          Prices are per million tokens (Mtok). Saving upserts by model name — an
          existing entry for this model is overwritten.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Model name">
          <Input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="gpt-4o"
            disabled={!!existing}
          />
        </Field>
        <Field label="Input price per Mtok">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={inputPerMtok}
            onChange={(e) => setInputPerMtok(e.target.value)}
          />
        </Field>
        <Field label="Output price per Mtok">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={outputPerMtok}
            onChange={(e) => setOutputPerMtok(e.target.value)}
          />
        </Field>
        <Field label="Cached input price per Mtok (optional)">
          <Input
            type="number"
            min={0}
            step="0.000001"
            value={cachedInputPerMtok}
            onChange={(e) => setCachedInputPerMtok(e.target.value)}
            placeholder="defaults to input price"
          />
        </Field>
        <Field label="Currency">
          <Input value={currency} onChange={(e) => setCurrency(e.target.value)} />
        </Field>
        {submit.isError && (
          <p className="text-xs text-destructive">{(submit.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={
            !model.trim() ||
            !inputPerMtok.trim() ||
            !outputPerMtok.trim() ||
            submit.isPending
          }
          onClick={() => submit.mutate()}
        >
          Save
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
