import { describe, expect, test } from "bun:test";

import { renderSnippet, SNIPPET_LANGS, type SnippetRequest } from "./snippets";

const REQ: SnippetRequest = {
  model: "llama-3.1-8b",
  messages: [{ role: "user", content: "hello there" }],
};
// the public base URL saved on Client Settings: the only address a snippet gets
const SAVED = { url: "https://gateway.example.com" };

describe("renderSnippet", () => {
  // the whole point is pasting it into an app, so the route name and the prompt
  // have to survive into every language
  test("every language carries the route and the prompt", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, REQ, SAVED);
      expect(out).toContain("llama-3.1-8b");
      expect(out).toContain("hello there");
      expect(out).toContain("https://gateway.example.com/v1");
    }
  });

  // a snippet is pasted into tickets and chat. inlining the operator's live
  // virtual key would leak a working credential every time
  test("no language inlines a credential", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, REQ, SAVED);
      expect(out).toContain("ROLTER_API_KEY");
      expect(out).not.toContain("sk-rolter-");
    }
  });

  // /gw needs a dashboard session, which a client outside the browser has not
  // got, so no snippet may point at it (#2486)
  test("no language points an external client at the /gw proxy", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, REQ, SAVED);
      expect(out).not.toContain("/gw");
      expect(out).not.toContain("in production");
    }
  });

  test("curl addresses the chat endpoint under the base, not beside it", () => {
    expect(renderSnippet("curl", REQ, SAVED)).toContain(
      "curl https://gateway.example.com/v1/chat/completions",
    );
  });

  test("curl sends a body the gateway would accept", () => {
    expect(curlBody(renderSnippet("curl", REQ, SAVED))).toEqual({
      model: "llama-3.1-8b",
      messages: [{ role: "user", content: "hello there" }],
    });
  });

  test("streaming changes the call shape, not just a flag", () => {
    const streamed = { ...REQ, stream: true };
    expect(curlBody(renderSnippet("curl", streamed, SAVED)).stream).toBe(true);
    expect(renderSnippet("python", streamed, SAVED)).toContain("for chunk in stream");
    expect(renderSnippet("javascript", streamed, SAVED)).toContain("for await");
  });

  test("non-streaming reads the message rather than a delta", () => {
    expect(renderSnippet("python", REQ, SAVED)).toContain("response.choices[0].message.content");
    expect(renderSnippet("javascript", REQ, SAVED)).toContain(
      "response.choices[0].message.content",
    );
  });

  // a prompt with quotes or newlines must not break out of the literal it sits
  // in — this is the case a naive template would get wrong
  test("a prompt with quotes and newlines stays escaped", () => {
    const nasty: SnippetRequest = {
      model: "gpt-4o",
      messages: [{ role: "user", content: 'say "hi"\nthen stop' }],
    };
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, nasty, SAVED);
      // the raw newline never reaches the literal; it is escaped
      expect(out).toContain("\\n");
      expect(out).toContain('\\"hi\\"');
    }
    expect(curlBody(renderSnippet("curl", nasty, SAVED)).messages[0].content).toBe(
      'say "hi"\nthen stop',
    );
  });

  // a chat column sends its whole thread, so the snippet has to carry every
  // turn, in order, or it reproduces a request the column never made (#2058)
  const THREAD: SnippetRequest = {
    model: "llama-3.1-8b",
    messages: [
      { role: "user", content: "my name is Anya" },
      { role: "assistant", content: "Nice to meet you, Anya." },
      { role: "user", content: "what is my name?" },
    ],
  };

  test("every language carries every turn of the conversation, in order", () => {
    for (const lang of SNIPPET_LANGS) {
      const out = renderSnippet(lang, THREAD, SAVED);
      const at = THREAD.messages.map((m) => out.indexOf(m.content));
      expect(at.every((i) => i >= 0)).toBe(true);
      expect([...at].sort((a, b) => a - b)).toEqual(at);
      expect(out).toContain('"assistant"');
    }
    expect(curlBody(renderSnippet("curl", THREAD, SAVED)).messages).toEqual(THREAD.messages);
    expect(curlBody(renderSnippet("curl", { ...THREAD, stream: true }, SAVED)).stream).toBe(true);
  });

  test("each turn is a line of its own", () => {
    for (const lang of SNIPPET_LANGS) {
      const lines = renderSnippet(lang, THREAD, SAVED).split("\n");
      for (const m of THREAD.messages) {
        expect(lines.filter((line) => line.includes(m.content))).toHaveLength(1);
      }
    }
  });

  // an apostrophe in a reply ("it's") would close curl's single-quoted body
  // and hand the rest of the JSON to the shell
  test("an apostrophe in any turn survives curl's single quotes", () => {
    const quoted: SnippetRequest = {
      model: "gpt-4o",
      messages: [
        { role: "user", content: "what's up?" },
        { role: "assistant", content: "It's fine, isn't it?" },
      ],
    };
    const out = renderSnippet("curl", quoted, SAVED);
    expect(out).toContain(`'\\''`);
    expect(curlBody(out).messages).toEqual(quoted.messages);
  });
});

describe("a turn that carried an image (#2955)", () => {
  const pictured: SnippetRequest = {
    model: "gpt-4o",
    messages: [
      { role: "user", content: "what is in this picture?", image: { mime: "image/png" } },
      { role: "assistant", content: "A cat." },
      { role: "user", content: "and the colour?" },
    ],
  };
  const DATA_URL = /data:image\/png;base64,[A-Za-z0-9+/=]{20,}/;

  test("curl keeps the content parts and reads the file into the url", () => {
    const out = renderSnippet("curl", pictured, SAVED);
    expect(out).toContain("IMAGE_B64=$(base64 < image.png | tr -d '\\n')");
    expect(out).toContain(`'"$IMAGE_B64"'`);
    const body = curlBody(out, "<b64>");
    expect(body.messages).toEqual([
      {
        role: "user",
        content: [
          { type: "text", text: "what is in this picture?" },
          { type: "image_url", image_url: { url: "data:image/png;base64,<b64>" } },
        ],
      },
      { role: "assistant", content: "A cat." },
      { role: "user", content: "and the colour?" },
    ]);
  });

  test("python and javascript read the file and keep the parts", () => {
    const py = renderSnippet("python", pictured, SAVED);
    expect(py).toContain("import base64");
    expect(py).toContain('open("image.png", "rb")');
    expect(py).toContain('"type": "image_url"');
    expect(py).toContain('f"data:image/png;base64,{image}"');
    const js = renderSnippet("javascript", pictured, SAVED);
    expect(js).toContain('import { readFileSync } from "node:fs"');
    expect(js).toContain('readFileSync("image.png").toString("base64")');
    expect(js).toContain('type: "image_url"');
    expect(js).toContain("`data:image/png;base64,${image}`");
  });

  test("no language inlines a data url", () => {
    for (const lang of SNIPPET_LANGS) {
      expect(renderSnippet(lang, pictured, SAVED)).not.toMatch(DATA_URL);
    }
  });

  test("a text-only request imports and reads nothing", () => {
    expect(renderSnippet("python", REQ, SAVED)).not.toContain("base64");
    expect(renderSnippet("javascript", REQ, SAVED)).not.toContain("readFileSync");
    expect(renderSnippet("curl", REQ, SAVED)).not.toContain("base64");
  });

  test("several images get a file and a variable each", () => {
    const two: SnippetRequest = {
      model: "gpt-4o",
      messages: [
        { role: "user", content: "a", image: { mime: "image/png" } },
        { role: "user", content: "b", image: { mime: "image/jpeg" } },
      ],
    };
    const out = renderSnippet("curl", two, SAVED);
    expect(out).toContain("IMAGE_B64_1=$(base64 < image-1.png");
    expect(out).toContain("IMAGE_B64_2=$(base64 < image-2.jpg");
    expect(renderSnippet("python", two, SAVED)).toContain("image_2 = base64");
    expect(renderSnippet("javascript", two, SAVED)).toContain("const image2 = readFileSync");
  });
});

/**
 * The JSON body of a curl snippet, read back the way a POSIX shell reads the
 * single-quoted `-d` argument: `'\''` is a quote closed, an escaped quote, and
 * the quote opened again.
 */
function curlBody(out: string, picture = "") {
  const quoted = out.slice(out.indexOf("\n  -d '") + 7, out.lastIndexOf("'"));
  // the splice of a picture variable stands in for the base64 the file holds
  const spliced = quoted.replace(/'"\$IMAGE_B64\w*"'/g, picture);
  return JSON.parse(spliced.split(`'\\''`).join("'")) as {
    model: string;
    messages: { role: string; content: unknown }[];
    stream?: boolean;
  };
}
