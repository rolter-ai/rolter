-- guardrail policies are consumed by the data plane and must invalidate the
-- control-plane snapshot version in the same transaction (#459)
create trigger guardrail_policies_bump_config_version
after insert or update or delete on guardrail_policies
for each statement execute function bump_config_version();

create trigger guardrail_policy_providers_bump_config_version
after insert or update or delete on guardrail_policy_providers
for each statement execute function bump_config_version();
