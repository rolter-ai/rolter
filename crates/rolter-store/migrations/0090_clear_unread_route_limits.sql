-- #2936: a route's rpm, tpm and concurrency are read by the gateway again.
--
-- the three sat in routes.advanced -> 'limits' for years and nothing read
-- them (#2924 removed them from the dashboard and the api, and the api has
-- dropped them from every write since). a row written before then can still
-- carry one, a number someone typed into a form that did nothing. the gateway
-- now enforces these keys, so a stale value would start refusing traffic the
-- day this release is deployed, on a cap nobody has looked at in months.
--
-- clearing them makes enforcement opt-in: a cap is in force only because
-- someone set it after the release that reads it. the statement trigger on
-- routes (0003) bumps config_version, so gateways pick the change up.
update routes
set advanced = advanced #- '{limits,rpm}' #- '{limits,tpm}' #- '{limits,concurrency}'
where advanced -> 'limits' ?| array['rpm', 'tpm', 'concurrency'];
