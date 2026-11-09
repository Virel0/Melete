-- An artist's picture of their own: the image in the folder above an album's folder (artist.jpg,
-- artist.png, folder.jpg or folder.png), relative to the library, or NULL when that folder is
-- the library's root or has none. Scans keep it current, as they do album covers; an artist
-- whose present albums all have the same one has it as their portrait.
ALTER TABLE music_albums ADD COLUMN artist_file TEXT;
