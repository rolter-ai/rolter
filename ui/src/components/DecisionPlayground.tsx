import * as React from "react";
import { useTranslation } from "react-i18next";

import { CodeBlock } from "@/components/ui/code-block";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Segmented } from "@/components/ui/segmented";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  DECISION_IMAGE_MAX_BYTES,
  DECISION_IMAGE_MAX_PIXELS,
  DECISION_IMAGE_TYPES,
  decisionRequest,
  pickDecisionModel,
} from "@/lib/decisions";
import { describeError } from "@/lib/error-copy";
import { decide, FAKE_MODEL, type DecisionResponse } from "@/lib/gateway";
import { useFormat } from "@/lib/i18n/format";

interface Props {
  models: { id: string }[];
  blocked: boolean;
  reason?: string;
}

/** Decision inputs stay mounted across tabs, like the other paid Playground calls. */
export function DecisionPlayground({ models, blocked, reason }: Props) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const [chosen, setChosen] = React.useState(FAKE_MODEL);
  // a configured route named fake-llm shadows the builtin, which then drops out
  // of `models`: the picker falls back to what is on offer rather than showing
  // a value that is not among its options
  const model = pickDecisionModel(models, chosen, FAKE_MODEL);
  const stateFormatLabel = React.useId();
  const [state, setState] = React.useState("");
  const [jsonState, setJsonState] = React.useState(false);
  const [questions, setQuestions] = React.useState(() =>
    JSON.stringify(
      {
        decision: { type: "noul", instructions: t("pages.playground.decisions.sampleQuestion") },
      },
      null,
      2,
    ),
  );
  const [image, setImage] = React.useState<string>();
  const [imagePending, setImagePending] = React.useState(false);
  const [imageInvalid, setImageInvalid] = React.useState(false);
  const imageInput = React.useRef<HTMLInputElement>(null);
  const [error, setError] = React.useState<string>();
  const [result, setResult] = React.useState<DecisionResponse>();
  const [busy, setBusy] = React.useState(false);
  const abort = React.useRef<AbortController | null>(null);
  const reader = React.useRef<FileReader | null>(null);
  const generation = React.useRef(0);
  React.useEffect(
    () => () => {
      generation.current++;
      abort.current?.abort();
      reader.current?.abort();
    },
    [],
  );

  const attach = async (file: File | undefined) => {
    const version = ++generation.current;
    reader.current?.abort();
    setImage(undefined);
    setImagePending(false);
    setImageInvalid(false);
    setError(undefined);
    if (!file) return;
    const reject = () => {
      if (version !== generation.current) return;
      setImagePending(false);
      setImageInvalid(true);
      setError(
        t("pages.playground.decisions.imageInvalid", {
          size: fmt.number(DECISION_IMAGE_MAX_BYTES / (1024 * 1024)),
          pixels: fmt.number(DECISION_IMAGE_MAX_PIXELS / 1_000_000),
        }),
      );
    };
    if (!DECISION_IMAGE_TYPES.includes(file.type) || file.size > DECISION_IMAGE_MAX_BYTES) {
      reject();
      return;
    }
    setImagePending(true);
    const next = new FileReader();
    reader.current = next;
    next.onerror = reject;
    next.onload = async () => {
      if (version !== generation.current) return;
      try {
        const bitmap = await createImageBitmap(file);
        const valid = bitmap.width * bitmap.height <= DECISION_IMAGE_MAX_PIXELS;
        bitmap.close();
        if (!valid) throw new Error("image");
        if (version === generation.current) {
          setImage(String(next.result));
          setImagePending(false);
        }
      } catch {
        reject();
      }
    };
    next.readAsDataURL(file);
  };

  const run = async () => {
    if (blocked || busy || imagePending || imageInvalid || !model) return;
    let request;
    try {
      request = decisionRequest(model, state, jsonState, questions, image);
    } catch {
      setError(t("pages.playground.decisions.invalid"));
      return;
    }
    setError(undefined);
    setResult(undefined);
    setBusy(true);
    const controller = new AbortController();
    abort.current = controller;
    try {
      const answer = await decide(request, controller.signal);
      if (!controller.signal.aborted) setResult(answer);
    } catch (e) {
      if (!controller.signal.aborted) setError(describeError(e, t).message);
    } finally {
      setBusy(false);
    }
  };

  // nothing on offer serves decisions: say so instead of a picker with nothing in it
  if (!models.length) {
    return <EmptyState inline description={t("pages.playground.decisions.noModels")} />;
  }

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <div className="flex min-w-0 flex-col gap-3">
        <Field label={t("pages.playground.decisions.model")}>
          <Combobox
            value={model}
            onChange={setChosen}
            options={models.map((m) => ({ value: m.id, label: m.id }))}
          />
        </Field>
        <p className="text-xs text-muted-foreground">{t("pages.playground.decisions.protocol")}</p>
        <div className="space-y-1.5">
          <FieldLabel label={t("pages.playground.decisions.stateFormat")} id={stateFormatLabel} />
          <Segmented<"text" | "json">
            labelledBy={stateFormatLabel}
            value={jsonState ? "json" : "text"}
            onChange={(v) => setJsonState(v === "json")}
            options={[
              { value: "text", label: t("pages.playground.decisions.text") },
              { value: "json", label: "JSON" },
            ]}
          />
        </div>
        <Field
          label={t("pages.playground.decisions.state")}
          hint={t("pages.playground.decisions.stateHint")}
        >
          <Textarea rows={5} value={state} onChange={(e) => setState(e.target.value)} />
        </Field>
        <Field
          label={t("pages.playground.decisions.questions")}
          hint={t("pages.playground.decisions.questionsHint")}
        >
          <Textarea
            rows={10}
            className="font-mono text-xs"
            value={questions}
            onChange={(e) => setQuestions(e.target.value)}
            spellCheck={false}
          />
        </Field>
        <Field
          label={t("pages.playground.decisions.image")}
          hint={t("pages.playground.decisions.imageHint", {
            size: fmt.number(DECISION_IMAGE_MAX_BYTES / (1024 * 1024)),
            pixels: fmt.number(DECISION_IMAGE_MAX_PIXELS / 1_000_000),
          })}
        >
          <Input
            ref={imageInput}
            type="file"
            accept={DECISION_IMAGE_TYPES.join(",")}
            onChange={(e) => void attach(e.target.files?.[0])}
          />
        </Field>
        {image && (
          <div className="flex items-start gap-2">
            <img
              src={image}
              alt={t("pages.playground.decisions.image")}
              className="max-h-40 max-w-full rounded-md border border-border"
            />
            <Button
              variant="outline"
              onClick={() => {
                generation.current++;
                reader.current?.abort();
                setImage(undefined);
                setImagePending(false);
                setImageInvalid(false);
                if (imageInput.current) imageInput.current.value = "";
              }}
            >
              {t("pages.playground.decisions.removeImage")}
            </Button>
          </div>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            onClick={() => void run()}
            title={reason}
            disabled={blocked || busy || imagePending || imageInvalid || !model}
          >
            {busy ? t("pages.playground.decisions.running") : t("pages.playground.decisions.run")}
          </Button>
          {busy && (
            <Button variant="outline" onClick={() => abort.current?.abort()}>
              {t("pages.playground.decisions.stop")}
            </Button>
          )}
        </div>
        {error && (
          <p role="alert" className="text-sm text-[color:var(--status-danger-text)]">
            {error}
          </p>
        )}
      </div>
      <div className="flex min-w-0 flex-col gap-3" aria-live="polite" aria-busy={busy}>
        {!result && !busy && <EmptyState description={t("pages.playground.decisions.empty")} />}
        {result &&
          Object.entries(result.answers ?? {}).map(([id, answer]) => (
            <section
              key={id}
              className="min-w-0 rounded-md border border-border p-3"
              aria-label={id}
            >
              <h3 className="break-all font-mono text-sm">{id}</h3>
              {answer.error ? (
                <CodeBlock
                  label={t("pages.playground.decisions.questionError")}
                  language="json"
                  value={JSON.stringify(answer.error, null, 2)}
                />
              ) : (
                <>
                  {answer.choice !== undefined && <p className="break-words">{answer.choice}</p>}
                  {answer.score !== undefined && (
                    <p>
                      {t("pages.playground.decisions.score", { value: fmt.number(answer.score) })}
                    </p>
                  )}
                  {answer.noul !== undefined && (
                    <p>
                      {t("pages.playground.decisions.yes", {
                        value: fmt.number(answer.noul * 100),
                      })}
                    </p>
                  )}
                  {answer.confidence !== undefined && (
                    <p className="text-xs text-muted-foreground">
                      {t("pages.playground.decisions.confidence", {
                        value: fmt.number(answer.confidence * 100),
                      })}
                    </p>
                  )}
                  {Object.entries(answer.probabilities ?? {}).map(([option, probability]) => (
                    <p key={option} className="break-words text-sm">
                      {answer.legend?.[option] ?? option}: {fmt.number(probability * 100)}%
                    </p>
                  ))}
                </>
              )}
            </section>
          ))}
        {result && (
          <CodeBlock
            label={t("pages.playground.decisions.raw")}
            language="json"
            value={JSON.stringify(result, null, 2)}
          />
        )}
      </div>
    </div>
  );
}
