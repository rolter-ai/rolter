-- several guardrail providers may run side by side now that scoped policies
-- pick which of them judge a request (#459); the one-enabled index from 0052
-- would refuse the second
drop index if exists guardrail_providers_one_enabled;
