-- Optimistic transaction version for Get -> modify -> Update across documents.
-- The compare-and-swap also serializes topology validation and outbox writes.
CREATE TABLE control_version (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    version bigint NOT NULL DEFAULT 0
);
INSERT INTO control_version (id, version) VALUES (true, 0);
