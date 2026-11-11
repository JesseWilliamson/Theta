# theta-drivers

PostgreSQL, MySQL/MariaDB and SQLite implementations of `theta_core::Driver` /
`Connection`, SSH tunnelling, and a demo-data seeder. Everything runs on Tokio.

```rust
let conn = theta_drivers::connect(&config).await?;   // opens an SSH tunnel first when config.ssh is set
let out = conn.execute("SELECT 1", &ExecOptions::default()).await?;
```

| Database | Client library | Notes |
| --- | --- | --- |
| PostgreSQL | `tokio-postgres` + rustls | binary wire format, portals for row limits, PostGIS as EWKB |
| MySQL / MariaDB | `mysql_async` (rustls) | text protocol, `KILL QUERY` cancellation, GEOMETRY converted to EWKB |
| SQLite | `rusqlite` (bundled) | one worker thread per connection, `sqlite3_interrupt` cancellation |
| SSH | `russh` | password, key file (+passphrase), ssh-agent; loopback listener + `direct-tcpip` |

## Behaviour worth knowing

* **One statement per `execute`.** PostgreSQL and SQLite enforce it themselves.
  `mysql_async` always enables multi-statements, so the MySQL driver refuses text
  with a second statement after a `;` (except `CREATE`/`ALTER`, whose routine and
  trigger bodies legitimately contain `;`).
* **Row limits** stop the server producing rows: PostgreSQL fetches `limit + 1`
  rows from a portal (wrapped in a transaction that is committed, or left alone
  when the user already has one open); MySQL kills the running query once it has
  one row too many.
* **Time zones.** `timestamptz` values are rendered in UTC with an explicit `+00`
  offset. MySQL `TIMESTAMP`/`DATETIME` have no zone and are shown as stored.
* **MariaDB JSON** is `LONGTEXT` on the wire, so result cells are `Value::Text`;
  introspection recognises JSON columns through their `json_valid` CHECK.
* **Geometry** is always `Value::Geometry(EWKB)`. MySQL's internal
  `SRID (4 bytes LE) + WKB` is rewritten to EWKB (SRID flag + SRID after the type
  word); SRID 0 yields plain WKB.
* **SSH host keys** are checked against `~/.ssh/known_hosts` with trust-on-first-use:
  unknown hosts are accepted and recorded, a *changed* key is refused.
* **A lost session** (the server closed the socket, the network dropped) is reported as
  `Error::Closed`. The application reconnects when it sees that error. A server that ends the session
  with an explicit error (PostgreSQL `57P01`, `terminating connection due to administrator command`)
  reports that error first; the next statement then fails with `Error::Closed`.
* **Plan parsers** are public functions (`parse_postgres_plan_json`, `parse_mysql_plan_json`,
  `parse_sqlite_plan`). PostgreSQL and MariaDB plans are tested against captured output *and* live servers;
  MySQL 8 `EXPLAIN FORMAT=JSON` and `EXPLAIN ANALYZE` are tested from the fixtures in `tests/fixtures/` only,
  because the live test server is MariaDB 10.11.
* `in_transaction` asks the server (`now() = statement_timestamp()` on
  PostgreSQL, the OK packet's status flags on MySQL), because neither client
  library exposes the protocol's transaction state. While a statement is running
  it returns the last known value instead of waiting.

## Seeding demo data

```sh
cargo run -p theta-drivers --example seed -- postgres [postgres://user:pass@host:port/db]
cargo run -p theta-drivers --example seed -- mysql    [mysql://user:pass@host:port/db]
cargo run -p theta-drivers --example seed -- sqlite   quarry_demo.sqlite
```

Without a URL the local test servers are used. Seeding drops and recreates the
demo tables (`customers`, `products`, `orders`, `order_items`, `stores`,
`delivery_zones`, `routes`, the `customer_order_summary` view, and `order_total()`
on PostgreSQL/MySQL) in the connection's *current* schema/database only.

## Tests

```sh
cargo test -p theta-drivers
```

Unit tests need nothing. The integration tests expect the local services from
`AGENTS.md` (PostgreSQL + PostGIS at `127.0.0.1:5432`, MariaDB at `127.0.0.1:3306`)
and create/drop their own uniquely named schemas and databases, so they can run
concurrently with other processes. SQLite tests use `:memory:` and temp files.

### SSH tests

They need a real sshd, so they are skipped unless `THETA_TEST_SSH=1`:

```sh
crates/theta-drivers/tests/ssh/setup.sh          # as root: keys, test user, sshd on 127.0.0.1:2222
THETA_TEST_SSH=1 cargo test -p theta-drivers --test ssh
crates/theta-drivers/tests/ssh/setup.sh stop
```

CI runs these tests too (the `libraries` job installs `openssh-server` and uses `THETA_TEST_SSH_DIR=/tmp/theta-ssh-test`;
the key directory must be world-traversable because `sshd` reads `authorized_keys` as the test user).

`setup.sh` generates host/client keys (ed25519, passphrase-protected ed25519, RSA,
and one unauthorised key), creates the user `thetatest` (password `theta-test-pw`)
and runs `/usr/sbin/sshd -f <dir>/sshd_config -p 2222` with password and public-key
authentication. Everything lives under `THETA_TEST_SSH_DIR`
(default `${TMPDIR:-/tmp}/theta-ssh-test`); `THETA_TEST_SSH_PORT` changes the port.
