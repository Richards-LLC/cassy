### Fixed — Named journey startup

- **Memory admission precedes server startup** — Named and responsive Playwright journeys prepare their source-bound snapshot before the webServer startup deadline begins. Refused memory admission exits with code 75 and a typed NOT_STARTED outcome. Ready and prebuilt snapshots still use normal source/hash verification; fixture and registered-hub runs keep their own servers and timeouts.
