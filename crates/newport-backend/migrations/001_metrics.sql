-- Compatible with the existing metrics.sqlite3 schema; retain all saved samples.
CREATE TABLE IF NOT EXISTS samples (
    server_id TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    at INTEGER NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY(server_id, endpoint, at)
);
CREATE INDEX IF NOT EXISTS samples_age ON samples(at);
