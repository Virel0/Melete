-- An artist known only by name whose albums joined the one MusicBrainz artist of that name
-- (compared as search keys compare names). The row stays, so its old address and pins still
-- lead somewhere: to the artist it joined. Worked out again after every music scan; NULL when
-- it stands on its own.
ALTER TABLE music_artists ADD COLUMN merged_into INTEGER REFERENCES music_artists (id);
