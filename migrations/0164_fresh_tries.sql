-- Songs and albums given up on before Soulseek was searched for songs with no search on
-- record, and before YouTube's matching read a missing leading article and Cyrillic spellings
-- and a refused video gave way to the next: their tries are forgotten, so each is tried again
-- with what Melete now knows, the newest wanted first. Soulseek searches again once a day has
-- passed since the last search or download, three times; YouTube once Soulseek is done.
DELETE FROM song_youtube WHERE state IN ('none', 'failed');
DELETE FROM soulseek_again;
