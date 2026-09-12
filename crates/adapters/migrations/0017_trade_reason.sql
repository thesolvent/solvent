-- Why a declined or failed trade never filled. Nullable: the happy path has no reason, and trades
-- that terminated before this column existed cannot be explained retroactively.
ALTER TABLE trade ADD COLUMN reason TEXT;
