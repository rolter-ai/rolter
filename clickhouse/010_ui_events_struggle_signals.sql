-- Three struggle signals the UX stream could not express (#1731).
--
-- The action enum covered what the operator *did*. It had no way to say what
-- they could not do, and each of these is a distinct kind of friction that
-- previously left no trace at all:
--
--   retry_submit   a save that failed and was attempted again. The second
--                  attempt used to be another form_submit, so the retry had to
--                  be reconstructed from timestamps -- and a retry is the
--                  moment somebody did not understand why the first one failed.
--   refused_click  a reach for a control RBAC denies. The dashboard renders
--                  gated controls disabled with a title saying why, so a reader
--                  repeatedly reaching for something they cannot have was
--                  invisible. target carries '<control-key>:<resource>:<action>'
--                  -- the control and the capability that refused it, both
--                  structural. No label, no message, no identity beyond the
--                  session and scope ids every row here already carries.
--   abandon_dirty  a form closed after being filled in. form_abandon stays the
--                  clean case (a misclick); this is somebody who tried and gave
--                  up, which is a different finding.
--
-- Appended, never reordered: an Enum8 ordinal is how the value is stored, so
-- renumbering the existing ten would rewrite the meaning of every row already
-- written rather than migrating it.

alter table ui_events
    modify column action Enum8(
        'screen_view'        = 1,
        'time_to_interactive'= 2,
        'navigate'           = 3,
        'back_out'           = 4,
        'form_submit'        = 5,
        'form_abandon'       = 6,
        'validation_error'   = 7,
        'empty_state'        = 8,
        'error_state'        = 9,
        'save_confirmed'     = 10,
        'retry_submit'       = 11,
        'refused_click'      = 12,
        'abandon_dirty'      = 13
    );
