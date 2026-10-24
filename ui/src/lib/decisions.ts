import type { DecisionRequest } from "@/lib/gateway";

/** OpenJev accepts one image shared by every question, never a remote URL. */
export const DECISION_IMAGE_MAX_BYTES = 4 * 1024 * 1024;
export const DECISION_IMAGE_TYPES = ["image/jpeg", "image/png", "image/webp"];

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
