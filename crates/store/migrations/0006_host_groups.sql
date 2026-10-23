-- Config v3: hosts have host groups instead of tags.
ALTER TABLE hosts RENAME COLUMN tags TO groups;
