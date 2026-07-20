import { useMutation } from "@tanstack/react-query";
import * as React from "react";

import { CopyButton } from "@/components/CopyButton";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import {
  createProvider,
  PROVIDER_KINDS,
  updateProvider,
  type ProviderRow,
} from "@/lib/api";

export type ProviderSheetMode = "add" | "edit";

interface ProviderDraft {
  name: string;
  slug: string;
  kind: string;
  apiBase: string;
  apiKey: string;
  apiKeyEnv: string;
  egressProxy: string;
}

function blankDraft(): ProviderDraft {
  return {
    name: "",
    slug: "",
    kind: PROVIDER_KINDS[0],
    apiBase: "",
    apiKey: "",
    apiKeyEnv: "",
    egressProxy: "",
  };
}

function fromProvider(p: ProviderRow): ProviderDraft {
  return {
    name: p.name,
    slug: p.slug,
    kind: p.kind,
    apiBase: p.api_base,
    apiKey: "",
    apiKeyEnv: p.api_key_env ?? "",
    egressProxy: p.egress_proxy ?? "",
  };
}

export interface ProviderSheetProps {
  open: boolean;
  mode: ProviderSheetMode;
  onOpenChange: (open: boolean) => void;
  orgId: string | null;
  provider?: ProviderRow | null;
  onDone: (created?: ProviderRow) => void;
}

export function ProviderSheet({
  open,
  mode,
  onOpenChange,
  orgId,
  provider,
  onDone,
}: ProviderSheetProps) {
  const [draft, setDraft] = React.useState<ProviderDraft>(() => blankDraft());
  const initialRef = React.useRef("");

  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current) return;
    seededRef.current = true;
    const d = mode === "edit" && provider ? fromProvider(provider) : blankDraft();
    setDraft(d);
    initialRef.current = JSON.stringify(d);
  }, [open, mode, provider]);

  const set = (patch: Partial<ProviderDraft>) => setDraft((d) => ({ ...d, ...patch }));

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const guard = React.useCallback(() => {
    if (!dirty) return true;
    return window.confirm("Discard unsaved changes?");
  }, [dirty]);

  // edit mode uses the backend's tri-state semantics: omit a field to leave it
  // unchanged, send "" to clear it, send a value to set/rotate it. api_key is
  // left out entirely unless the operator typed a new one — never pre-filled,
  // so an empty submit must not clear a credential that's just not being rotated
  const save = useMutation({
    mutationFn: () => {
      if (mode === "add") {
        return createProvider(orgId as string, {
          name: draft.name,
          slug: draft.slug.trim() || undefined,
          kind: draft.kind,
          api_base: draft.apiBase,
          api_key: draft.apiKey || undefined,
          api_key_env: draft.apiKeyEnv || undefined,
          egress_proxy: draft.egressProxy || undefined,
        });
      }
      const p = provider!;
      return updateProvider(p.id, {
        kind: draft.kind !== p.kind ? draft.kind : undefined,
        api_base: draft.apiBase !== p.api_base ? draft.apiBase : undefined,
        api_key: draft.apiKey ? draft.apiKey : undefined,
        api_key_env: draft.apiKeyEnv !== (p.api_key_env ?? "") ? draft.apiKeyEnv : undefined,
        egress_proxy:
          draft.egressProxy !== (p.egress_proxy ?? "") ? draft.egressProxy : undefined,
      });
    },
    onSuccess: (created) => {
      onDone(created);
      onOpenChange(false);
    },
  });

  const title = mode === "add" ? "Add provider" : `Edit ${provider?.name ?? ""}`;
  const subtitle =
    mode === "add"
      ? "an upstream provider used as a route target"
      : `${draft.slug || "—"} · ${draft.kind}`;
  const cta = mode === "add" ? "Create provider" : "Save provider";
  const canSave =
    !!draft.name.trim() && !!draft.apiBase.trim() && !save.isPending &&
    (mode === "add" ? !!orgId : true);

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader
        title={title}
        subtitle={subtitle}
        onClose={() => guard() && onOpenChange(false)}
      />
      <SheetBody>
        <p className="text-xs leading-snug text-muted-foreground">
          {mode === "add"
            ? "Providers are scoped to the current org and used as route targets and provider-group members."
            : "Leave the API key blank to keep the stored credential unchanged. Clear the env var or egress proxy field to unset it."}
        </p>

        <Field label="Name">
          <Input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="openai-primary"
            disabled={mode === "edit"}
          />
        </Field>

        {mode === "add" ? (
          <Field
            label="Slug (optional)"
            hint="URL-safe id for provider-slug/model addressing; derived from the name if blank, and immutable after create"
          >
            <Input
              value={draft.slug}
              onChange={(e) => set({ slug: e.target.value })}
              placeholder="openai-primary"
              className="font-mono"
            />
          </Field>
        ) : (
          <Field label="Slug" hint="immutable identity for provider-slug/model addressing">
            <div className="flex items-center gap-2">
              <Input value={draft.slug} readOnly disabled className="font-mono" />
              {provider && <CopyButton value={`${provider.slug}/`} label="Copy address prefix" />}
            </div>
          </Field>
        )}

        <Field label="Kind">
          <Select value={draft.kind} onChange={(e) => set({ kind: e.target.value })}>
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </Select>
        </Field>

        <Field label="API base">
          <Input
            value={draft.apiBase}
            onChange={(e) => set({ apiBase: e.target.value })}
            placeholder="https://api.openai.com/v1"
          />
        </Field>

        <Field
          label="API key (optional)"
          hint={
            mode === "add"
              ? "sealed at rest; never displayed again"
              : "blank leaves the stored key unchanged; sealed at rest, never displayed"
          }
        >
          <Input
            type="password"
            value={draft.apiKey}
            onChange={(e) => set({ apiKey: e.target.value })}
            autoComplete="off"
            placeholder={mode === "edit" ? "unchanged" : undefined}
          />
        </Field>

        <Field label="API key env var (optional)" hint="read from this env var instead">
          <Input
            value={draft.apiKeyEnv}
            onChange={(e) => set({ apiKeyEnv: e.target.value })}
            placeholder="OPENAI_API_KEY"
          />
        </Field>

        <Field label="Egress proxy (optional)">
          <Input
            value={draft.egressProxy}
            onChange={(e) => set({ egressProxy: e.target.value })}
            placeholder="http://proxy.internal:8080"
          />
        </Field>
      </SheetBody>

      <SheetFooter>
        {save.isError && (
          <p className="px-[22px] pt-2.5 text-xs text-destructive">
            {(save.error as Error).message}
          </p>
        )}
        <div className="flex items-center justify-end gap-2.5 px-[22px] py-3.5">
          <Button variant="ghost" onClick={() => guard() && onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!canSave} onClick={() => save.mutate()}>
            {cta}
          </Button>
        </div>
      </SheetFooter>
    </Sheet>
  );
}
