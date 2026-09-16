# Custom CA bundles

Rolter can add private CA certificates to the normal public-root trust store for outbound HTTPS connections to upstream providers. Certificate-chain and hostname verification remain enabled; this feature does not affect inbound TLS or configure mTLS client certificates.

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
