#!/usr/bin/env bash
# the one throwaway postgres every worktree on this machine runs its tests
# against (#1736). rolter_store::postgres::test_database already gives each
# worktree a database of its own on the server (#1430), so a container per
# worktree buys no isolation and only leaks a port, memory and disk once the
# worktree is gone. see "The Postgres test database" in
# docs/dev-docs/development/testing.md
#
#   test-postgres.sh up              start it if needed, print the export line
#   test-postgres.sh url             print ROLTER_TEST_DATABASE_URL for it
#   test-postgres.sh status          connections in use and the worktree databases
#   test-postgres.sh release <path>  drop the test database of the worktree at <path>
#   test-postgres.sh down            remove the container and its data
#
# `up` prints progress on stderr and only the export line on stdout, so
# `eval "$(scripts/test-postgres.sh up)"` sets the variable in the calling shell
set -euo pipefail

name=rolter-test-pg
port="${ROLTER_TEST_PG_PORT:-55433}"
# the same major as the postgres service the CI test jobs run against
image=postgres:16-alpine
# headroom for several worktrees running the postgres suites at once; the
# measurement behind the number is under "The connection budget" in testing.md
max_connections="${ROLTER_TEST_PG_MAX_CONNECTIONS:-300}"

die() {
    echo "test-postgres: $*" >&2
    exit 1
}

have_docker() {
    command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1
}

running() {
    [ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || true)" = true ]
}

psql_admin() {
    docker exec -i "$name" psql -U postgres -d postgres -v ON_ERROR_STOP=1 -X -q "$@"
}

# the url for the running container, from the port it was actually published
# on rather than the one this invocation would pick
url() {
    local published
    published=$(docker port "$name" 5432/tcp 2>/dev/null | head -n 1 || true)
    echo "postgres://postgres:postgres@${published:-127.0.0.1:$port}/rolter_test"
}

up() {
    have_docker || die "docker is not available"
    local state
    state=$(docker inspect -f '{{.State.Status}}' "$name" 2>/dev/null || true)
    case "$state" in
        running) ;;
        "")
            echo "starting $name on 127.0.0.1:$port (max_connections=$max_connections)" >&2
            # fsync and friends are off because nothing here outlives a test run;
            # the data directory is the container's own, so `down` takes it along
            docker run -d --name "$name" \
                --label org.rolter.role=test-postgres \
                --shm-size 256m \
                -p "127.0.0.1:$port:5432" \
                -e POSTGRES_PASSWORD=postgres \
                -e POSTGRES_DB=rolter_test \
                "$image" \
                -c "max_connections=$max_connections" \
                -c fsync=off -c synchronous_commit=off -c full_page_writes=off \
                >/dev/null
            ;;
        *)
            echo "starting the existing $name" >&2
            docker start "$name" >/dev/null
            ;;
    esac
    # the image runs a socket-only server while it initialises, so readiness over
    # tcp is readiness of the real one
    local _
    for _ in $(seq 1 60); do
        if docker exec "$name" pg_isready -U postgres -h 127.0.0.1 -q 2>/dev/null; then
            echo "export ROLTER_TEST_DATABASE_URL=$(url)"
            return 0
        fi
        sleep 1
    done
    die "$name did not become ready; see: docker logs $name"
}

status() {
    if ! have_docker || ! running; then
        die "$name is not running; start it with: just test-pg"
    fi
    echo "url: $(url)"
    psql_admin -tA <<'SQL'
select 'connections: ' || count(*) filter (where backend_type = 'client backend') - 1
              || ' client backends in use of max_connections '
              || current_setting('max_connections')
    from pg_stat_activity;
SQL
    echo "worktree databases (the worktree each belongs to):"
    psql_admin -tA -F '  ' <<'SQL'
select datname, coalesce(shobj_description(oid, 'pg_database'), '(no worktree recorded)')
    from pg_database
  where datname like 'rolter\_test\_wt\_%'
  order by datname;
SQL
}

# drop every database the worktree at $1 owns. the test harness records that
# path as the database's comment, which is what this matches on. it runs as a
# blocking worktrunk pre-remove hook, so it never fails: a server that is down
# or a database still in use leaves the drop to the lazy sweep the next test
# run in any worktree performs
release() {
    local path=${1:-}
    [ -n "$path" ] || die "usage: test-postgres.sh release <worktree-path>"
    path=${path%/}
    if ! have_docker || ! running; then
        return 0
    fi
    psql_admin -tA -v path="$path" <<'SQL' || true
select 'dropping test database ' || datname || ' of ' || :'path'
    from pg_database
  where datname like 'rolter\_test\_wt\_%'
      and shobj_description(oid, 'pg_database') = :'path';
select format('drop database if exists %I', datname)
    from pg_database
  where datname like 'rolter\_test\_wt\_%'
      and shobj_description(oid, 'pg_database') = :'path'
\gexec
SQL
    return 0
}

down() {
    have_docker || die "docker is not available"
    if docker inspect "$name" >/dev/null 2>&1; then
        docker rm -f -v "$name" >/dev/null
        echo "removed $name" >&2
    fi
}

case "${1:-up}" in
    up) up ;;
    url) url ;;
    status) status ;;
    release) release "${2:-}" ;;
    down) down ;;
    *) die "unknown command ${1}; expected up, url, status, release or down" ;;
esac
