### Fixed — QA ledger

- **A ledger written on another machine passes the evidence check once its files are restored** — Was: a ledger row that cited its capture by the writing machine's absolute path (for example a prowl `/Users/…/.cas/artifacts/<project>/<task>/log` path) was refused at merge as "PASS capture is missing, unreadable or outside the ledger directory", although the same file sat in the local task directory. Now: such a path is found at the same place under the local task directory, and it must still resolve inside the ledger directory. The ledger file is not rewritten.
