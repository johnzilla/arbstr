-- Request log for cost tracking and observability
CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    correlation_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    model TEXT NOT NULL,
    provider TEXT,
    policy TEXT,
    streaming BOOLEAN NOT NULL DEFAULT FALSE,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats REAL,
    provider_cost_sats REAL,
    latency_ms INTEGER NOT NULL,
    success BOOLEAN NOT NULL,
    error_status INTEGER,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_requests_correlation_id ON requests(correlation_id);
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);

-- Learned input/output ratios per policy (populated in future phases)
CREATE TABLE IF NOT EXISTS token_ratios (
    policy TEXT PRIMARY KEY,
    avg_ratio REAL NOT NULL,
    sample_count INTEGER NOT NULL DEFAULT 0
);
