# Dev fixtures

Throwaway MySQL 8 for the milestone checks. Rootless podman — no sudo after
the one-time `apt install podman`.

```bash
# Start (first run pulls the image)
podman run -d --name db-query-mysql \
  -e MYSQL_ROOT_PASSWORD=devpassword \
  -p 3306:3306 docker.io/library/mysql:8

# Wait for it to accept connections, then seed
podman exec -i db-query-mysql \
  mysql -uroot -pdevpassword < dev/seed.sql
```

Connect from the app with host `127.0.0.1`, port `3306`, user `root`,
password `devpassword`, database `poc`.

**Tick "Allow invalid certificates".** The container generates a self-signed
server cert, so the default `VerifyIdentity` mode will (correctly) refuse it.
That refusal is itself worth seeing once — it proves the secure default works.

```bash
podman stop db-query-mysql && podman rm db-query-mysql   # tear down
```

## What each fixture is for

| Object | Proves |
|---|---|
| `types_zoo` | Every `CellValue` decode arm; row 2 straddles the 2^53 BIGINT boundary; row 3 is all-NULL; `c_literal_null` holds the *string* `"NULL"` so it must render differently from a real null |
| `big` (7500 rows) | Auto-LIMIT truncates at 5000 and the chip appears |
| `users` / `orders` | Joins, aliases, autocomplete, and DECIMAL money that must not go through `f64` |
| `user_totals` | The tree's VIEW vs BASE TABLE distinction |
