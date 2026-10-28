# Kubernetes and Helm

The supported chart is in [`charts/rolter`](../../../charts/rolter). It deploys separate gateway and control-plane workloads and services, with health probes, hardened pod defaults, resource requests, an optional HPA, disruption budget, and ingress.

```bash
helm upgrade --install rolter ./charts/rolter --namespace rolter --create-namespace \
  --set env.databaseUrl='postgres://rolter:secret@postgres.example/rolter' \
  --set env.redisUrl='redis://redis.example:6379'
```

PostgreSQL, Redis, and ClickHouse are external by design. Supply provider credentials through Kubernetes Secrets using `secretEnv`; do not place credentials in `config.file` or Helm values committed to source control. For GitOps-managed configuration, create a ConfigMap containing `rolter.toml` and set `config.existingConfigMap`.

The control plane, the gateways and both preflight containers read that one file (`/etc/rolter/rolter.toml`). The control plane used to read the image's baked example instead, whose public `sk-rolter-dev` key it then served to every gateway through the snapshot (#2408). Keep `[[virtual_keys]]` out of the file: `rolter check`, which preflight runs, fails on that key.

The gateway defaults to two replicas. The control plane defaults to one replica; scale it only after validating the database migration and UI-hosting behavior for your deployment. Configure TLS at the ingress or service-mesh boundary.

## Image tags per component

Both Deployments run `image.tag`, or the chart's `appVersion` when that is empty. `control.image.tag` and `gateway.image.tag` override it for one component alone (#3352), and each Deployment's preflight init container runs its own component's image. That lets an upgrade follow ADR-0038's order, the control plane first and the gateways after it, and lets the gateways be held on an older release or rolled back without the control plane. A gateway may lag behind its control plane but never lead it. `helm template` coverage is the `image-tags` case of `scripts/check-helm-chart.sh`; the operator procedure is in `docs/user-docs/deployment/upgrading.mdx` ("Two steps with the Helm chart").
