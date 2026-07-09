# rolter

High-performance OpenAI/Anthropic-compatible LLM gateway and load balancer.

`rolter` is the unified command-line launcher for the [rolter](https://github.com/ormeilu/rolter)
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

See the [project README](https://github.com/ormeilu/rolter#readme) for architecture,
configuration and deployment docs.

## License

Apache-2.0
