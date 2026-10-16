CREATE TABLE IF NOT EXISTS communication_slo_summaries (
    factory_session TEXT PRIMARY KEY,
    observed_at TEXT NOT NULL,
    summary_json TEXT NOT NULL
);
