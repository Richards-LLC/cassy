# Neon — Common Queries & Branch Routing

Use the IDs from `SKILL.md`'s `<!-- keep neon-ids -->` block.
Install `neonctl` and authenticate with `neonctl auth` before these commands.

## Branch routing and SQL

Choose the intended branch explicitly; production queries should be read-only.

```bash
neonctl branches list --project-id "<projectId>" --output json
neonctl connection-string "<branchId>" --project-id "<projectId>" \
  --database-name "<databaseName>" --psql -- -c "SELECT 1;"
```

For schema inspection, replace the query with
`SELECT table_name, column_name, data_type FROM information_schema.columns WHERE table_schema = 'public';`.
For query plans, use `EXPLAIN SELECT ...`; `EXPLAIN ANALYZE` executes the query.

## Migration (3-step flow)

1. Create a disposable branch from the intended parent:
   `neonctl branches create --project-id "<projectId>" --parent "<branchId>" --name "migration-check" --output json`.
2. Apply and verify the migration on its returned branch ID:
   `neonctl connection-string "<tempBranchId>" --project-id "<projectId>" --database-name "<databaseName>" --psql -- -v ON_ERROR_STOP=1 -f migration.sql`.
   Compare schemas with
   `neonctl branches schema-diff "<branchId>" "<tempBranchId>" --project-id "<projectId>" --database "<databaseName>"`.
3. Apply the reviewed migration through the project's deployment process to the
   intended branch, with explicit approval for production. Creating or comparing
   branches does not promote SQL changes. Delete the temporary branch afterward.

## Branch lifecycle

```bash
neonctl branches get "<branchId>" --project-id "<projectId>" --output json
neonctl branches delete "<tempBranchId>" --project-id "<projectId>"
```

Only delete disposable branches you created. Resetting a branch discards changes;
confirm the branch and approval before `neonctl branches reset "<branchId>"
--parent --project-id "<projectId>"`.

## Connection strings

Use `neonctl connection-string "<branchId>" --project-id "<projectId>"
--database-name "<databaseName>"` when another tool needs a URL. It contains a
password: do not echo it into shared logs or commit it. Prefer `--psql -- -c ...`
for queries so the connection URL stays out of captured command output.
