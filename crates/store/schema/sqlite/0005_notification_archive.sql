-- A notification its reader put away (the inbox's archive): when they did, 0 while it
-- is in the inbox. One put away is read; more of it within the hour brings it back,
-- unread, as it makes one that was read unread.
ALTER TABLE notifications ADD COLUMN archived_at BIGINT NOT NULL DEFAULT 0;
CREATE INDEX notifications_inbox_idx ON notifications (recipient_id, updated_at DESC, id DESC) WHERE archived_at = 0;
