-- Request-log sampling keeps only a fraction of rows, so a plain count() or
-- sum() over `request_logs` under-reports traffic and spend (#2239).
--
-- `sample_weight` is 1 / sample_rate at write time: how many real requests the
-- row stands for. Analytics scale counts and sums by it (never averages or
-- percentiles). Defaults to 1, the weight of every row written unsampled and
-- of every row written before this migration.

alter table request_logs
    add column if not exists sample_weight Float64 default 1;
