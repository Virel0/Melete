-- Why a file went to the trash: replaced by an upgrade, or set aside by an admin as the wrong
-- file for its title (then replaced_by is empty).
ALTER TABLE trashed_files ADD COLUMN reason TEXT NOT NULL DEFAULT 'upgrade'
    CHECK (reason IN ('upgrade', 'wrong'));
