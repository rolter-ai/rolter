import type { DecisionRequest } from "@/lib/gateway";

/** OpenJev accepts one image shared by every question, never a remote URL. */
export const DECISION_IMAGE_MAX_BYTES = 4 * 1024 * 1024;
/** The same contract caps the decoded image at 12 million pixels, whatever its byte size. */
export const DECISION_IMAGE_MAX_PIXELS = 12_000_000;
export const DECISION_IMAGE_TYPES = ["image/jpeg", "image/png", "image/webp"];

/**
 * The model the picker holds: the one chosen while it is still offered, else
 * the built-in `fake-llm`, else the first model on offer, else none. A configured
 * route named `fake-llm` shadows the builtin and drops out of the decision
 * models, so a fixed default would name something the picker does not list.
 */
export function pickDecisionModel(
  models: readonly { id: string }[],
  chosen: string,
  builtin: string,
): string {
  if (models.some((m) => m.id === chosen)) return chosen;
  if (models.some((m) => m.id === builtin)) return builtin;
  return models[0]?.id ?? "";
}

/** Keep JSON state as JSON and preserve question ids and vendor extensions. */
export function decisionRequest(
  model: string,
  state: string,
  stateIsJson: boolean,
  questions: string,
  image?: string,
): DecisionRequest {
  const parsedState: unknown = stateIsJson ? JSON.parse(state) : state;
  if (parsedState === null || !["string", "object"].includes(typeof parsedState)) {
    throw new Error("state");
  }
  const parsed: unknown = JSON.parse(questions);
  if (
    !parsed ||
    typeof parsed !== "object" ||
    Array.isArray(parsed) ||
    !Object.keys(parsed).length
  ) {
    throw new Error("questions");
  }
  for (const [id, question] of Object.entries(parsed)) {
    if (!id.trim() || !question || typeof question !== "object" || Array.isArray(question)) {
      throw new Error("questions");
    }
    const q = question as Record<string, unknown>;
    if (
      typeof q.type !== "string" ||
      typeof q.instructions !== "string" ||
      !q.instructions.trim()
    ) {
      throw new Error("questions");
    }
  }
  return {
    model,
    state: parsedState as DecisionRequest["state"],
    questions: parsed as DecisionRequest["questions"],
    ...(image ? { image_data: image } : {}),
  };
}
