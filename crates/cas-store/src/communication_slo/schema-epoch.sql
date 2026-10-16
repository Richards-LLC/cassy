CREATE TABLE IF NOT EXISTS communication_observation_epoch AS
SELECT 1 AS singleton, strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS started_at;
