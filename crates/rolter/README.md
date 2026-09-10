# rolter

High-performance OpenAI/Anthropic-compatible LLM gateway and load balancer.

`rolter` is the unified command-line launcher for the [rolter](https://github.com/rolter-ai/rolter)
system. A single binary dispatches to both planes:

```console
# data-plane gateway (openai/anthropic-compatible proxy + load balancer)
rolter gateway --config rolter.toml

# control plane (management api + static dashboard host)
rolter control --database-url postgres://localhost/rolter
```

Install from crates.io:

```console
cargo install rolter
```

or as a Python-managed CLI (maturin wheel):

```console
uv tool install rolter
```

See the [project README](https://github.com/rolter-ai/rolter#readme) for architecture,
configuration and deployment docs.

## Compatibility

This crate is a **binary**. The `rolter-*` library crates it depends on are
published only so `cargo install rolter` can resolve, and they offer no stable
Rust API — any public item in them may change or disappear in any release.

The surfaces that do carry a compatibility promise are rolter's HTTP APIs, its
configuration file and its environment variables. What each of them guarantees,
and the deprecation window before anything is removed, is in
[Versioning & compatibility](https://github.com/rolter-ai/rolter/blob/master/user-docs/community/versioning.mdx).

## License

Apache-2.0
