-- value redaction for captured payloads (#1835). `payload_capture_redact_fields`
-- only blanks the value of a JSON key it knows by name; these two columns add
-- detectors that scan the text of every captured string. the default enables
-- every built-in detector (names mirror rolter_core::payload_redaction), so a
-- deployment that has capture on starts redacting credential shapes without an
-- operator having to opt in. the `logging_settings_bump_config_version`
-- statement trigger from 0034 already fires on any update of this table, so the
-- new columns need no trigger of their own
alter table logging_settings
    add column if not exists payload_capture_redact_detectors text[] not null default array[
        'private_key', 'anthropic_key', 'openai_key', 'google_api_key', 'github_token',
        'slack_token', 'stripe_key', 'aws_key', 'jwt', 'bearer_token', 'connection_string'
    ],
    add column if not exists payload_capture_redact_patterns jsonb not null default '[]'::jsonb;
