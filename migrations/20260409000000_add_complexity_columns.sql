-- Add complexity scoring columns to requests table.
-- Both nullable: rows predating the scorer remain NULL.
ALTER TABLE requests ADD COLUMN complexity_score REAL;
ALTER TABLE requests ADD COLUMN tier TEXT;
