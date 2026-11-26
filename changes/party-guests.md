### Added

- Watch party guests with no account: anyone in a party makes an invite link
  (`POST /api/v1/watch-parties/{id}/invites`) that lets one guest in once, within a day. The
  guest gives a name and gets a pass for that party alone (`/api/v1/party`): they follow it,
  play, pause and seek, get their own stream or conversion of what plays with its subtitles,
  and leave; they see the title, the other guests and how many from the household are
  watching, never an account's name. Their pass and links end with the party, when they leave,
  or when anyone in it sends them away. A party's guest cap holds their streams. A way in may
  open `party` alone (`MELETE_WAYS_IN`), so guests reach their party through it and nothing of
  the household.
