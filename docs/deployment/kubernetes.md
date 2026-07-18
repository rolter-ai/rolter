# Kubernetes and Helm

The supported chart is in [`charts/rolter`](../../charts/rolter). It deploys separate gateway and control-plane workloads and services, with health probes, hardened pod defaults, resource requests, an optional HPA, disruption budget, and ingress.

```bash
helm upgrade --install rolter ./charts/rolter --namespace rolter --create-namespace \
  --set env.databaseUrl='postgres://rolter:secret@postgres.example/rolter' \
  --set env.redisUrl='redis://redis.example:6379'
```

PostgreSQL, Redis, and ClickHouse are external by design. Supply provider credentials through Kubernetes Secrets using `secretEnv`; do not place credentials in `config.file` or Helm values committed to source control. For GitOps-managed configuration, create a ConfigMap containing `rolter.toml` and set `config.existingConfigMap`.

The gateway defaults to two replicas. The control plane defaults to one replica; scale it only after validating the database migration and UI-hosting behavior for your deployment. Configure TLS at the ingress or service-mesh boundary.

