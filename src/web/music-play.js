// What the Listen pages play: an album, tracks from anywhere, in order or shuffled, as the
// player's queue entries ({ id, title, artist, album, album_id, cover, codec, duration_seconds }).
//
// - albumQueue(album): an album's tracks, in its track-list order, each with its cover.
// - looseQueue(tracks): tracks from anywhere in the library; they carry no cover of their own.
// - play(queue, from) plays from an entry; shufflePlay(queue) turns shuffle on and starts
//   anywhere in it, the rest mixed (Shuffle off in the bar puts it back in order).
// - playAlbum(id) asks for an album and plays it from the start.

import { api } from "/web/lib.js";
import { playQueue } from "/web/music-player.js";
import { shuffled, toggleShuffle } from "/web/music-queue.js";

export function albumQueue(album) {
  return album.track_list.map((t) => ({
    id: t.id,
    title: t.title,
    artist: t.artist ?? album.artist,
    album: album.title,
    album_id: album.id,
    cover: album.cover,
    codec: t.codec,
    duration_seconds: t.duration_seconds,
  }));
}

export function looseQueue(tracks) {
  return tracks.map((t) => ({
    id: t.id,
    title: t.title,
    artist: t.artist ?? t.album_artist,
    album: t.album,
    album_id: t.album_id,
    cover: null,
    codec: t.codec,
    duration_seconds: t.duration_seconds,
  }));
}

export function play(queue, from = 0) {
  if (queue.length) playQueue(queue, from);
}

export function shufflePlay(queue) {
  if (!queue.length) return;
  if (!shuffled()) toggleShuffle();
  playQueue(queue, Math.floor(Math.random() * queue.length));
}

export async function playAlbum(id) {
  const album = await api(`/music/albums/${id}`);
  play(albumQueue(album));
  return album;
}
