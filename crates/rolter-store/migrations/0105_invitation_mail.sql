-- Invitation delivery by email (#1828).
--
-- An invitation used to be a link the creating admin was handed and had to
-- pass on by hand. With a mail transport configured the control plane sends it
-- to the invitee, and the outcome is kept on the row, so a failed send is
-- visible next to the invitation instead of vanishing into a log. A failure
-- never fails the invitation: the link is still returned to the admin.
--
-- Like the rest of `invitations`, the data plane never reads this, so there is
-- deliberately no bump_config_version() trigger.

alter table invitations
    -- 'not_configured': no mail transport was set up when the invitation was
    -- created (every invitation that predates this migration reads that way)
    add column if not exists mail_status text not null default 'not_configured',
    add column if not exists mail_attempted_at timestamptz,
    -- a short reason safe to show an admin: never the server's reply, which may
    -- name the recipient, and never the transport's credentials
    add column if not exists mail_error text;

do $$
begin
    alter table invitations
        add constraint invitations_mail_status_check
        check (mail_status in ('not_configured', 'sent', 'failed'));
exception
    when duplicate_object then null;
end
$$;
