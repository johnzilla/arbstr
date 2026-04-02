-- Pending settlements for vault billing reconciliation.
--
-- When a vault settle or release call fails after retries,
-- the operation is recorded here for later replay by the
-- background reconciliation task.
CREATE TABLE IF NOT EXISTS pending_settlements (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL CHECK(type IN ('settle', 'release')),
    reservation_id TEXT NOT NULL UNIQUE,
    amount_msats INTEGER,
    metadata TEXT NOT NULL,
    created_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_pending_settlements_created
    ON pending_settlements(created_at);
