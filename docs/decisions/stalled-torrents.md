Decision: A torrent Deluge says is downloading whose progress hasn't gone up for 4 hours (since it
was added, or last moved) is set aside and its title searched for again, its release passed
over. When another release is grabbed, the stalled torrent is removed from Deluge with its data
on the seedbox and its release blocklisted; otherwise it goes back to downloading, its release
off the blocklist again, and is looked at again after another 4 hours. Paused and queued
torrents aren't stalled; Soulseek downloads keep their own handling.

Why: A release with too few seeders can sit at 0% for days while the title waits on it; a rare
title shouldn't lose the only copy anyone has, so it's only dropped for another one.

Date: 2026-11-06
