# Using a PostgreSQL connection

Each connection granted to you is a libpq **service**; its name is listed with the account in
`guide.md`. There is no database password for you: the connection goes through the Aurora gateway,
which holds the real login and checks every statement. Do not look for or ask for a password.

```sh
psql service=<name> -c 'select current_database(), current_user'
psql service=<name> -c '\dt'
psql service=<name> -At -F, -c 'select id, name from customers limit 20'
```

Any PostgreSQL driver works with the service file the environment already points at
(`PGSERVICEFILE`). Send **one statement at a time**, each under 1 MiB.

## The four classes of statement

| Class | Statements | By default |
|---|---|---|
| `read` | SELECT, SHOW, EXPLAIN | allowed |
| `write` | INSERT, UPDATE, DELETE | held for a person's approval |
| `ddl` | CREATE, ALTER, DROP, TRUNCATE, GRANT | held for a person's approval |
| `admin` | VACUUM, ANALYZE, SET, LISTEN, DO, CALL and functions not on the allow list | held for a person's approval |

`tools.md` shows what your grant allows. A refused or held statement answers with SQLSTATE `42501` and
the reason; tell the person what was refused and why instead of rewording the statement to get around it.

Never permitted, whatever the grant: `ALTER SYSTEM`, `SET ROLE`, `COPY` to a program or a file,
large-object file functions, `dblink` and file-reading functions.

## Habits

- Start with `\dt`, `\d <table>` and `select ... limit 20`; never `select *` on a big table without
  `limit`.
- Read first, then change: show the person the rows an `UPDATE` or `DELETE` will touch (the same
  `WHERE` in a `select count(*)`) before you run it.
- Do not retry a held write: it waits for approval; report that it is pending.
- Save large results to a file (`\o /tmp/out.txt` or redirect `psql` output) and read the first lines.
