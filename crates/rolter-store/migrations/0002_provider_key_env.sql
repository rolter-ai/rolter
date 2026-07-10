-- reference an environment variable that holds a provider's upstream api key.
--
-- the `provider_keys` table (envelope-encrypted ciphertext) remains the
-- intended long-term home for upstream credentials once the secret-backend
-- work (kek from env/kms) lands; this column is the interim, env-based path
-- so the postgres-backed config store can resolve keys the same way the
-- bootstrap toml does today.
alter table providers add column if not exists api_key_env text;
