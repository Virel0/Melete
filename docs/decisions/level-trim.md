Decision: The web player levels volume by Melete's loudness measurement alone, with the apps'
arithmetic (down to −18 LUFS, never up, held under full scale), and trims the silence Melete
measured at each song's start and end. Both are on by default and kept per browser. A song not
measured yet plays as it is; when its measurement arrives mid-song, its level glides there.

Why: The apps fall back to a file's ReplayGain tags, which the web can't read; Melete measures
every track the same way, so its number is the one all devices share. Matching the apps'
rounding keeps one album at one level on every device.

Date: 2026-11-07
