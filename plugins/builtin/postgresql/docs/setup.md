# Connect a PostgreSQL database

Aurora lets agents run SQL on a PostgreSQL server **without ever holding its password**. You enter
a database login once; Aurora's gateway keeps it. An agent connects to the gateway (with `psql` or
any PostgreSQL driver), and the gateway opens the real connection itself. Every statement is
classified and checked against your policy before it reaches the server.

## Before you connect

1. Create a **database role for Aurora** with only the privileges agents need. A read-only role
   (`GRANT SELECT` on the tables concerned) is the safest start. Aurora's policy adds a second
   layer, but the role is the one that holds even if a rule is wrong.
2. Make sure the server accepts connections from the Aurora gateway. The platform's own database
   and other internal addresses are refused by the gateway.
3. Prefer `verify-full`. If the server uses a certificate from a private authority, paste that
   authority's certificate (PEM) in *CA certificate*.

## Connecting

Choose *PostgreSQL* under *Apps*, enter the server host, port (5432 by default), database, user and
password, pick the TLS mode and give the connection a name such as `orders prod`. The name becomes
the **service name** agents use (`orders-prod`); the connection page shows it.

Then grant the connection to the agents that may use it, like any other app.

## How an agent connects

The agent's workspace already holds the connection settings: the environment variable
`PGSERVICEFILE` points at a service file that Aurora maintains. The agent runs

```
psql service=orders-prod -c 'select 1'
```

and no secret appears on the command line or in any file: the service points at a small relay inside
the sandbox that logs in to the Aurora gateway for the agent, over TLS that it verifies against the
gateway's certificate authority. Neither your database password nor any gateway credential is
readable by the agent. Other PostgreSQL drivers can read the same service name
(`service=orders-prod`) through libpq.

## What agents can do

Each statement belongs to one class, and each class is a tool you can allow, ask about or block:

| Tool | Statements |
|---|---|
| Read data | `SELECT`, `SHOW`, `EXPLAIN` without `ANALYZE`, `COPY ... TO STDOUT` |
| Change data | `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `COPY ... FROM STDIN`, `SELECT ... FOR UPDATE` |
| Change the schema | `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `COMMENT`, `GRANT`, `REVOKE` |
| Administer | `VACUUM`, `ANALYZE`, most `SET`, `LISTEN`/`NOTIFY`, `DO`, `CALL`, functions that are not on the safe list |

Reading is allowed by default. Everything else is held for approval; the approval card shows the
statement with its literal values hidden. Transaction control (`BEGIN`, `COMMIT`, ...) is always
allowed. A statement with several parts is judged by its strictest part.

Some statements are **never** permitted, whatever the policy says: `ALTER SYSTEM`, `SET ROLE`,
`SET SESSION AUTHORIZATION`, `COPY` to or from a program or server file, `lo_import`/`lo_export`,
`dblink` and `postgres_fdw`, the `pg_read_file` family, and anything Aurora cannot parse. A refused
statement answers the agent with SQLSTATE `42501` and the reason.

## Limits to know

- Statements are limited to 1 MiB.
- The role you entered is what the server sees; Aurora does not create per-agent roles.
- Rate limits, approvals and the request log work as for any other app.
