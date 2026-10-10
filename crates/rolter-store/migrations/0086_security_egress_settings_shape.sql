-- #2943: the database refuses a security or egress setting the gateway could
-- only read by guessing.
--
-- three jsonb columns held a value the control plane writes in a fixed shape
-- but the table never checked:
--
--   security_settings.required_headers  an object of header name -> value, the
--                                       ingress rule "every request must carry
--                                       this header". read as no rule when it
--                                       was not an object, and an entry whose
--                                       value was not a string was dropped
--   providers.egress_proxies            an array of proxy urls (0018 checks the
--                                       array, not what is in it). an element
--                                       that was not a string loaded as an empty
--                                       pool, so the provider went out direct
--   client_settings.injected_headers    an object of header name -> value
--
-- the loader no longer reads any of them leniently (the first two fail closed,
-- the third degrades and says so), and the api already refuses what these
-- constraints refuse. this keeps a row written some other way, such as a seed,
-- an import or hand-run sql, out of the table too.
--
-- a header name must not be blank, and a required value must not be blank
-- either: the api refuses both, and a request can never carry a header whose
-- value is empty, so such a rule would lock the deployment out rather than
-- gate it. `strict` mode keeps jsonpath from unwrapping a nested array and
-- passing a value that is not a string; the `case` keeps a value that is not an
-- object from reaching `keyvalue()`, which raises on one.
--
-- a migration must not brick a database that already holds a bad row, which
-- the old api, or sql, could have written. every constraint is therefore added
-- `not valid`: it checks each insert and update from now on and leaves existing
-- rows alone. the block below then validates each one that can be, so the
-- usual database, with nothing wrong in it, ends up with fully validated
-- constraints. a constraint whose table holds a bad row stays `not valid` until
-- the operator corrects the row and runs `alter table <table> validate
-- constraint <name>`, which docs/user-docs/deployment/upgrading.mdx walks
-- through. until then the control plane refuses to publish a snapshot while
-- `required_headers` is unreadable, and leaves a provider whose
-- `egress_proxies` is unreadable out of it, saying which in
-- `GET /api/v1/config/problems`.
--
-- no new trigger: the three tables already bump config_version on every write
-- (0021, 0003 and 0061 respectively), and a constraint changes no row.

alter table security_settings
    add constraint security_settings_required_headers_shape
        check (
            case when jsonb_typeof(required_headers) = 'object'
                then not jsonb_path_exists(
                    required_headers,
                    'strict $.keyvalue() ? (@.key like_regex "^\\s*$"
                        || @.value.type() != "string"
                        || @.value like_regex "^\\s*$")'
                )
                else false
            end
        ) not valid;

alter table providers
    add constraint providers_egress_proxies_strings
        check (
            case when jsonb_typeof(egress_proxies) = 'array'
                then not jsonb_path_exists(
                    egress_proxies,
                    'strict $[*] ? (@.type() != "string")'
                )
                else false
            end
        ) not valid;

alter table client_settings
    add constraint client_settings_injected_headers_shape
        check (
            case when jsonb_typeof(injected_headers) = 'object'
                then not jsonb_path_exists(
                    injected_headers,
                    'strict $.keyvalue() ? (@.key like_regex "^\\s*$"
                        || @.value.type() != "string")'
                )
                else false
            end
        ) not valid;

do $$
declare
    shape record;
begin
    for shape in
        select * from (values
            ('security_settings', 'security_settings_required_headers_shape'),
            ('providers', 'providers_egress_proxies_strings'),
            ('client_settings', 'client_settings_injected_headers_shape')
        ) as t(table_name, constraint_name)
    loop
        begin
            execute format('alter table %I validate constraint %I',
                           shape.table_name, shape.constraint_name);
        exception when check_violation then
            raise notice '% constraint % holds a row that breaks it and stays not valid; see Upgrading in the Rolter docs',
                shape.table_name, shape.constraint_name;
        end;
    end loop;
end
$$;
