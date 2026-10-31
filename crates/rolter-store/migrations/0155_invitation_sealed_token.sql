-- Resending an invitation email (#3304).
--
-- An invitation row keeps only a peppered digest of its one-time token, so the
-- link cannot be rebuilt once the creating admin has been shown it. A resend
-- reuses the same link, which means the token has to be kept somewhere the
-- control plane can read back: sealed with the KEK, like every other secret the
-- control plane must use again later, and only while the invitation is live.
-- Accepting or revoking an invitation clears it, and so does replacing it.
--
-- Both columns are null for an invitation that predates this migration, was
-- created with no mail transport, or was created on a control plane with no KEK;
-- such an invitation cannot be resent and has to be revoked and sent again.
--
-- Like the rest of `invitations`, the data plane never reads this, so there is
-- deliberately no bump_config_version() trigger.

alter table invitations
    add column if not exists token_ciphertext bytea,
    add column if not exists token_nonce bytea;

do $$
begin
    alter table invitations
        add constraint invitations_token_seal_pair
        check ((token_ciphertext is null) = (token_nonce is null));
exception
    when duplicate_object then null;
end
$$;
