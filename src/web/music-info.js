// What a song's file is, for Get info in its actions strip: the format, bit rate, sample rate,
// bit depth, channels and spatial audio, its ISRC and its length, as the music API shows the
// track. A fact the file doesn't carry is left out.
//
// CODECS names a codec as people know it; the album page uses it for what its files are.

import { clock, h, number } from "/web/lib.js";
import { SPATIAL } from "/web/music-parts.js";

export const CODECS = {
  flac: "FLAC",
  alac: "ALAC",
  mp3: "MP3",
  aac: "AAC",
  opus: "Opus",
  vorbis: "Vorbis",
  wav: "WAV",
  aiff: "AIFF",
  wavpack: "WavPack",
  ape: "APE",
  musepack: "Musepack",
  speex: "Speex",
};

export const codecName = (codec) => CODECS[codec] ?? codec.toUpperCase();

function channelsName(channels) {
  if (channels === 1) return "Mono";
  if (channels === 2) return "Stereo";
  return `${number.format(channels)} channels`;
}

// The facts as [name, value] pairs, in the order shown.
export function songFacts(track) {
  const facts = [
    ["Format", track.codec ? codecName(track.codec) : null],
    ["Bit rate", track.bitrate_kbps ? `${number.format(track.bitrate_kbps)} kbit/s` : null],
    ["Sample rate", track.sample_rate ? `${number.format(track.sample_rate / 1000)} kHz` : null],
    ["Bit depth", track.bit_depth ? `${track.bit_depth}-bit` : null],
    ["Channels", track.channels ? channelsName(track.channels) : null],
    ["Spatial audio", track.spatial ? SPATIAL[track.spatial] : null],
    ["Length", track.duration_seconds ? clock(track.duration_seconds) : null],
    ["ISRC", track.isrc || null],
  ];
  return facts.filter(([, value]) => value);
}

// The facts as a list, or a sentence when the file says none of them.
export function songInfo(track) {
  const facts = songFacts(track);
  if (!facts.length) return h("p", { class: "note" }, "Nothing is known about this song's file yet.");
  return h("dl", { class: "kv song-info" }, facts.map(([name, value]) => [h("dt", {}, name), h("dd", {}, value)]));
}
