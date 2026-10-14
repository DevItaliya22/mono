-- 010_workspace_blob_origins_migration.sql

-- workspace.db 9 -> 10. Adds `blob_origins`, durable provenance learned from
-- peer advertisements: when an inventory says a peer holds a blob and names its
-- origin device, the receiver used to keep that only in a per-session in-memory
-- directory, so a freshly booted device with every holder offline had nothing
-- left to name in a missing-image placeholder. The `blobs` row's attributes
-- remain the record where the bytes are held; this table is the same fact for
-- devices that only ever heard about the blob.
--
-- First record wins: provenance is who *first* held the bytes, never last
-- touch, so a re-advertisement is `ON CONFLICT DO NOTHING` rather than an
-- update. Never used for routing — any holder may serve the bytes and the
-- digest decides — so a stale row costs nothing.

CREATE TABLE blob_origins (
  hash          TEXT    PRIMARY KEY,
  origin_device TEXT    NOT NULL,
  recorded_at   INTEGER NOT NULL
);
