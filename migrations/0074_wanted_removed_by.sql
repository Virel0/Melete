-- Who took a title off the wanted list, shown beside when. Unknown for those taken off
-- before this, and once that account is gone.
ALTER TABLE wanted_removed ADD COLUMN removed_by INTEGER REFERENCES users (id) ON DELETE SET NULL;
