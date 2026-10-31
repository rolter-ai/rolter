# Local ONNX fixtures

The small `model.onnx` graph is a deterministic token-id detector, not a trained
safety classifier. `model-f64.onnx` has a valid f64 output that the loader must
refuse. `model-empty.onnx` supports zero-id `[1, 0]` inputs so content-less calls
exercise native inference rather than relying on injected special tokens.
Regenerate these graphs and the small WordLevel tokenizer with:

```sh
uv run --python 3.12 --script make_fixture.py
```

Run `just fmt-docs` from the repository root afterward and update the manifest's
pins if the model or tokenizer bytes changed.

## Real tokenizer parity

`roberta-tokenizer.json.gz` contains the canonical tokenizer from `FacebookAI/roberta-base` at revision
`e2da8e2f811d1448a5b465c236feacd80ffbac7b`:

https://huggingface.co/FacebookAI/roberta-base/resolve/e2da8e2f811d1448a5b465c236feacd80ffbac7b/tokenizer.json

The upstream bytes have SHA-256
`847bbeab6174d66a88898f729d52fa8d355fafe1bea101cf960dd404581df70e`.
The parity test checks that exact digest after decompression. The gzip wrapper keeps
learned vocabulary data below the repository's file-size gate without changing it.
The model repository declares MIT; `LICENSE.roberta` retains the Facebook MIT
notice from the upstream fairseq project.

`make_tokenizer_vectors.py` uses the official `tokenizers==0.22.2` Python wheel
(onig backend) to generate `tokenizer-vectors.json`. Inputs are stored as repeat
recipes, and expected token ids as a count and SHA-256 of little-endian u32s.
Regenerate with `uv run --python 3.12 --script make_tokenizer_vectors.py`, then
`just fmt-docs`. Normal tests use these local files without network access.
The vectors cover Unicode, contractions, punctuation, long whitespace and a
million-space prefix. They check tokenization parity, not classifier quality.

A separate native regression uses a custom backtracking Split pattern. With
fancy-regex it silently lost the late injection and returned a benign score;
onig reported a retry-limit panic instead. The shipped Rust backend rejects
backtracking custom patterns at load, accepts linear patterns, and checks that
repeated calls still detect the fixture's needle. Parity on the real RoBERTa
tokenizer alone would not have caught that custom-pattern failure.
