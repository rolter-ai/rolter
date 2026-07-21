"""tenant + fake-vLLM fleet bootstrap, shared by every scenario.

the compose file runs three sim engines (sim-a/-b/-c). :func:`register_fleet`
wires N models across them behind routes and mints a virtual key, so a scenario
can start from "a fresh tenant with a working fleet" in one call.
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass, field

from .client import ControlClient

# engine service names on the compose network; the gateway reaches them here.
# api_base is the host root WITHOUT a /v1 suffix — the proxy appends the full
# /v1/... request path itself (mirrors the example toml: https://api.openai.com).
SIM_ENGINES = ["http://sim-a:8000", "http://sim-b:8000", "http://sim-c:8000"]
# every sim serves this model name (see docker-compose.e2e.yml `--model`)
UPSTREAM_MODEL = "rolter-dummy"


@dataclass
class Fleet:
    org_id: str
    team_id: str
    project_id: str
    provider_ids: list[str] = field(default_factory=list)
    route_ids: list[str] = field(default_factory=list)
    models: list[str] = field(default_factory=list)
    virtual_key: str | None = None


def new_tenant(admin: ControlClient, *, name: str | None = None) -> Fleet:
    org = admin.create_org(name=name)
    team = admin.create_team(org["id"])
    project = admin.create_project(team["id"])
    return Fleet(org_id=org["id"], team_id=team["id"], project_id=project["id"])


def register_fleet(admin: ControlClient, *, models: int = 24, tenant: Fleet | None = None,
                   strategy: str = "round_robin") -> Fleet:
    """create ``models`` routes spread across the three sim engines, each with a
    target per engine, then mint a virtual key scoped to every model.

    24 models × 3 targets ≈ the "20-30 llms as 3-4 clusters" shape from #613.
    """
    fleet = tenant or new_tenant(admin)

    # snapshot validation requires globally-unique provider names and route
    # models, so scope every name to this fleet with a short token.
    tok = uuid.uuid4().hex[:6]

    # one provider per sim engine, reused by every route target
    providers = [
        admin.create_provider(fleet.org_id, api_base=base, name=f"sim-{tok}-{i}")
        for i, base in enumerate(SIM_ENGINES)
    ]
    fleet.provider_ids = [p["id"] for p in providers]

    for n in range(models):
        model = f"e2e-model-{tok}-{n:02d}"
        route = admin.create_route(fleet.project_id, model, strategy=strategy)
        for p in providers:
            admin.add_route_target(route["id"], p["id"], upstream_model=UPSTREAM_MODEL)
        fleet.route_ids.append(route["id"])
        fleet.models.append(model)

    key = admin.create_virtual_key(fleet.project_id, models=fleet.models)
    fleet.virtual_key = key["key"]
    return fleet
