# rolter Helm chart

The chart deploys the data-plane gateway and control plane as separate workloads. PostgreSQL, Redis, and ClickHouse are deliberately external dependencies so production operators can use managed services and their normal backup policies.

```bash
helm upgrade --install rolter ./charts/rolter \
  --set env.databaseUrl='postgres://rolter:secret@postgres/rolter' \
  --set env.redisUrl='redis://redis:6379'
```

Provider credentials should be supplied with `secretEnv` and Kubernetes Secret references, never in `config.file` or command-line values:

```yaml
secretEnv:
  - name: OPENAI_API_KEY
    valueFrom:
      secretKeyRef:
        name: rolter-providers
        key: openai-api-key
```

Use `config.existingConfigMap` to manage `rolter.toml` outside the release. The gateway runs with a read-only root filesystem, no Linux capabilities, no service-account token, health probes, resource defaults, and a disruption budget. Enable `gateway.autoscaling` and `ingress` when the cluster provides metrics-server and an ingress controller.

