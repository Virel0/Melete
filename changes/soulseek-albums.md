### Changed

- **Add new says which albums Soulseek's users share.** In Manage → Music → Add new, while
  slskd is set up, the one Soulseek search made when typing stops now serves albums as well as
  songs: each of MusicBrainz's albums that users share as a whole album folder says by how
  many and in which formats, and the Albums list is then ranked by how close each is to the
  words, as Songs is. Only albums MusicBrainz knows are listed, and adding one works as before.
  Only the words go to slskd. For the apps, `POST /api/v1/requests/soulseek/candidates` (admins)
  takes the words and MusicBrainz's albums for them, and answers Soulseek's songs and the albums
  its users share, from one search.
