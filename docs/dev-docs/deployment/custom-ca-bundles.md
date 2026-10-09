# Custom CA bundles

Rolter can add private CA certificates to the normal public-root trust store for outbound HTTPS connections to upstream providers. Certificate-chain and hostname verification remain enabled; this feature does not affect inbound TLS or configure mTLS client certificates.

The public roots are the host's own trust store: the upstream client verifies certificates through `rustls-platform-verifier`, which on Linux reads the system CA bundle (`/etc/ssl/certs`, or the file `SSL_CERT_FILE` and the directory `SSL_CERT_DIR` name). The published image is distroless and ships `ca-certificates`, so it works unchanged; an image you build yourself on a base without one needs it installed. Before reqwest 0.13 the client carried a bundled copy of the Mozilla roots instead, so a deployment that relied on a minimal image with no system store must now install one or point `SSL_CERT_FILE` at a bundle. Postgres TLS (`sqlx`) and `wss://` realtime upstreams keep their bundled webpki roots.

All of these clients use one rustls crypto provider, aws-lc-rs: the one `reqwest` 0.13 ships, with `sqlx` and `ldap3` set to it. rustls selects its provider from the crate features and panics on the first `ClientConfig::builder()` that finds two, so a `wss://` dial would die at runtime on a build that compiles and passes every plain-http test. `.config/deny.toml` bans the `ring` crate to keep it that way, and `a_wss_dial_finds_its_crypto_provider` in the gateway's `realtime.rs` dials a `wss://` address in a workspace-wide test build.

## Minimal air-gapped configuration

Mount a PEM file containing one or more CA certificates, then use either the environment variable:

```sh
ROLTER_CA_BUNDLE=/etc/rolter/ca/private-root.pem rolter-gateway --config /app/rolter.toml
```

or the matching global TOML field:

```toml
[tls]
ca_bundles = ["/etc/rolter/ca/root.pem", "/etc/rolter/ca/intermediate.pem"]

[[providers]]
name = "private-vllm"
kind = "openai_compatible"
api_base = "https://llm.internal.example"
```

`ROLTER_CA_BUNDLE` replaces the global TOML list. A provider can replace the global selection independently:

```toml
[[providers]]
name = "isolated-cluster"
kind = "openai_compatible"
api_base = "https://llm.cluster.internal"
ca_bundles = ["/etc/rolter/ca/cluster-root.pem"]
```

Other providers keep using the global private roots plus the built-in public roots. Set a provider's `ca_bundles = []` to use public roots only.

## Docker Compose

Mount the bundle read-only and pass its in-container path:

```yaml
services:
  gateway:
    environment:
      ROLTER_CA_BUNDLE: /etc/rolter/ca/private-root.pem
    volumes:
      - ./pki/private-root.pem:/etc/rolter/ca/private-root.pem:ro
```

## Kubernetes

Store the public CA certificate in a ConfigMap or Secret and mount it read-only:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: rolter-upstream-ca
data:
  private-root.pem: |
    -----BEGIN CERTIFICATE-----
    ...
    -----END CERTIFICATE-----
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rolter-gateway
spec:
  template:
    spec:
      containers:
        - name: gateway
          image: rolter:latest
          env:
            - name: ROLTER_CA_BUNDLE
              value: /etc/rolter/ca/private-root.pem
          volumeMounts:
            - name: upstream-ca
              mountPath: /etc/rolter/ca
              readOnly: true
      volumes:
        - name: upstream-ca
          configMap:
            name: rolter-upstream-ca
```

## Validation and rotation

Startup fails with the bundle path and an actionable error when a configured file is missing, unreadable, contains no certificates, or has malformed PEM. Snapshot updates are rejected under the same conditions.

HTTP clients capture trust roots when their connection pool is created. After replacing a mounted certificate, publish or fetch a new configuration snapshot—even if the path is unchanged—to clear configured pools and rebuild them from the new bundle. With static bootstrap configuration, restart the gateway. Existing in-flight connections finish with their original trust configuration; subsequent connections use the rotated bundle.
