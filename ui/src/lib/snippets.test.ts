import { describe, expect, test } from "bun:test";

import { renderSnippet, snippetBaseUrl, SNIPPET_LANGS, type SnippetRequest } from "./snippets";

const REQ: SnippetRequest = { model: "llama-3.1-8b", prompt: "hello there" };
const ORIGIN = "https://rolter.localhost";

describe("snippetBaseUrl", () => {
  test("points at the gateway proxy under the dashboard origin", () => {
    expect(snippetBaseUrl(ORIGIN)).toBe("https://rolter.localhost/gw/v1");
  });

  test("does not double the slash when the origin carries one", () => {
    expect(snippetBaseUrl("https://rolter.localhost/")).toBe("https://rolter.localhost/gw/v1");
  });
});

describe("renderSnippet", () => {
  // the whole point is pasting it into an app, so the route name and the prompt
  // have to survive into every language
  test("every language carries the route and the prompt", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, REQ, ORIGIN);
      expect(out).toContain("llama-3.1-8b");
      expect(out).toContain("hello there");
      expect(out).toContain("https://rolter.localhost/gw/v1");
    }
  });

  // a snippet is pasted into tickets and chat. inlining the operator's live
  // virtual key would leak a working credential every time
  test("no language inlines a credential", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, REQ, ORIGIN);
      expect(out).toContain("ROLTER_API_KEY");
      expect(out).not.toContain("sk-rolter-");
    }
  });

  test("each language names the base url as the dashboard proxy", () => {
    for (const lang of SNIPPET_LANGS) {
      expect(renderSnippet(lang, REQ, ORIGIN)).toContain("in production");
    }
  });

  test("curl sends a body the gateway would accept", () => {
    const out = renderSnippet("curl", REQ, ORIGIN);
    const body = out.slice(out.indexOf("-d '") + 4, out.lastIndexOf("'"));
    expect(JSON.parse(body)).toEqual({
      model: "llama-3.1-8b",
      messages: [{ role: "user", content: "hello there" }],
    });
  });

  test("streaming changes the call shape, not just a flag", () => {
    const streamed = { ...REQ, stream: true };
    expect(renderSnippet("curl", streamed, ORIGIN)).toContain('"stream":true');
    expect(renderSnippet("python", streamed, ORIGIN)).toContain("for chunk in stream");
    expect(renderSnippet("javascript", streamed, ORIGIN)).toContain("for await");
  });

  test("non-streaming reads the message rather than a delta", () => {
    expect(renderSnippet("python", REQ, ORIGIN)).toContain("response.choices[0].message.content");
    expect(renderSnippet("javascript", REQ, ORIGIN)).toContain(
      "response.choices[0].message.content",
    );
  });

  // a prompt with quotes or newlines must not break out of the literal it sits
  // in — this is the case a naive template would get wrong
  test("a prompt with quotes and newlines stays escaped", () => {
    const nasty: SnippetRequest = {
      model: "gpt-4o",
      prompt: 'say "hi"\nthen stop',
    };
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, nasty, ORIGIN);
      // the raw newline never reaches the literal; it is escaped
      expect(out).toContain("\\n");
      expect(out).toContain('\\"hi\\"');
    }
    const curlOut = renderSnippet("curl", nasty, ORIGIN);
    const body = curlOut.slice(curlOut.indexOf("-d '") + 4, curlOut.lastIndexOf("'"));
    expect(JSON.parse(body).messages[0].content).toBe('say "hi"\nthen stop');
  });
});
