# rolter Helm chart

The full guide, with the values that matter, is in the [user docs](../../docs/user-docs/deployment/helm.mdx).

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

The control plane and the gateways run `image.tag` (empty means the chart's `appVersion`), unless `control.image.tag` or `gateway.image.tag` names another release for that component alone. That lets an upgrade move the control plane first and the gateways after it, and lets the gateways be held on, or rolled back to, an older release. A gateway may lag behind its control plane but never lead it. The two-step procedure is in the [upgrading guide](../../docs/user-docs/deployment/upgrading.mdx#two-steps-with-the-helm-chart).

Use `config.existingConfigMap` to manage `rolter.toml` outside the release. The gateway runs with a read-only root filesystem, no Linux capabilities, no service-account token, health probes, resource defaults, and a disruption budget. Enable `gateway.autoscaling` and `ingress` when the cluster provides metrics-server and an ingress controller.
