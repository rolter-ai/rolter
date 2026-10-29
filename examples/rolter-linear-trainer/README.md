# rolter-linear/1 trainer check

No service lives here, only a test (#3455). `test_documented_trainer.py` runs the
scikit-learn trainer printed in
[Semantic guard policies](../../docs/user-docs/security/semantic-guard-policies.mdx) over the
golden vectors in `crates/rolter-gateway/tests/fixtures/local_classifier/golden_features.json`.
The gateway's own unit test checks the same file, so a model trained with the documented
script scores what the gateway computes.

```sh
uv run --no-project --python 3.13 --with-requirements requirements-dev.txt pytest
```
