// What this browser plays, and so how the player plays a file: as it is, with only the container
// and audio converted, or converted whole. And how audio and subtitle tracks are named.

import { languageName } from "/web/lib.js";

// ---------- What this browser plays ----------

export const probe = document.createElement("video");
export const mse = window.ManagedMediaSource ?? window.MediaSource;
// Vendored, unchanged, from the hls.js 1.7.3 npm release. Loaded only when a
// conversion plays in a browser without HLS of its own.
export const HLS_MODULE = "/web/hls-1.7.3.mjs";

function videoCodec(v) {
  const tenBit = /10/.test(v.pixel_format ?? "");
  switch (v.codec) {
    case "h264":
      // Browsers decode 8-bit H.264 only.
      return tenBit ? null : "avc1.640033";
    case "hevc":
      return tenBit ? "hvc1.2.4.L153.B0" : "hvc1.1.6.L153.B0";
    case "av1":
      return tenBit ? "av01.0.08M.10" : "av01.0.08M.08";
    case "vp9":
      return tenBit ? "vp09.02.41.10" : "vp09.00.41.08";
    default:
      return null;
  }
}

const AUDIO_CODECS = {
  aac: "mp4a.40.2",
  mp3: "mp4a.6B",
  opus: "opus",
  flac: "flac",
  vorbis: "vorbis",
  ac3: "ac-3",
  eac3: "ec-3",
  alac: "alac",
};

function container(info) {
  const format = info.media?.container ?? "";
  if (format.includes("mp4")) return "video/mp4";
  // ffprobe calls Matroska and WebM alike; browsers play only the WebM kind.
  if (format.includes("webm") && info.path.toLowerCase().endsWith(".webm")) return "video/webm";
  return null;
}

// Dolby Vision without a fallback (profile 5) shows wrong colours anywhere but a Dolby Vision
// player, so it's always converted.
function wrongColours(v) {
  return v.dolby_vision && v.dolby_vision.fallback === null;
}

// Whether the file takes more than the way in to the server allows (its `max_bitrate`): then
// it plays only converted, which the server holds to the limit.
function overLimit(info) {
  const limit = info.max_bitrate;
  if (!limit) return false;
  const media = info.media;
  const bits = media?.bit_rate ?? (media?.duration_seconds ? (info.size * 8) / media.duration_seconds : null);
  return bits == null || bits > limit;
}

// How to play the file with these choices: { direct: true } or a transcode request.
export function choose(info, choice) {
  const media = info.media;
  // ffprobe couldn't read it; the browser may still manage, and a conversion is the fallback.
  if (!media) return choice.fallback ? { video_codec: "h264" } : { direct: true };
  const v = media.video[0];
  const defaultAudio = media.audio.find((a) => a.default) ?? media.audio[0];
  const audio = media.audio.find((a) => a.index === choice.audio) ?? defaultAudio;
  const request = (codec) => ({
    video_codec: codec,
    ...(audio ? { audio_stream: audio.index } : {}),
    ...(choice.burn ? { burn_subtitle: choice.burn } : {}),
    ...(choice.sound ? { audio_mode: choice.sound } : {}),
  });
  if (choice.burn || choice.fallback || !v || wrongColours(v) || overLimit(info)) return request("h264");
  const vc = videoCodec(v);
  const ac = audio ? AUDIO_CODECS[audio.codec] : null;
  const type = container(info);
  const codecs = [vc, audio ? ac : null].filter(Boolean).join(",");
  // A sound other than as mixed is made by the server, so the file can't play as it is.
  if (type && vc && (!audio || ac) && audio === defaultAudio && !choice.sound && probe.canPlayType(`${type}; codecs="${codecs}"`)) {
    return { direct: true };
  }
  // Only the container or the audio is the problem: keep the video as it is.
  const mp4 = `video/mp4; codecs="${vc}"`;
  const copyPlays = mse?.isTypeSupported(mp4) || probe.canPlayType("application/vnd.apple.mpegurl") && probe.canPlayType(mp4);
  if ((v.codec === "h264" || v.codec === "hevc") && vc && copyPlays) return request("copy");
  return request("h264");
}

export function describe(mode) {
  if (mode.direct) return "Playing the original file.";
  const sound = SOUNDS.find((s) => s.mode === mode.audio_mode && s.mode);
  const said = sound ? ` Sound: ${sound.label.toLowerCase()}.` : "";
  if (mode.burn_subtitle) return `Converting for this browser, with the subtitles drawn in.${said}`;
  if (mode.video_codec === "copy") return `Playing the original video; the server converts the container and audio.${said}`;
  return `Converting for this browser.${said}`;
}

// The sounds a conversion can have, as the server names them ("" for as mixed).
export const SOUNDS = [
  { mode: "", label: "As mixed" },
  { mode: "dialogue", label: "Clear dialogue" },
  { mode: "levelled", label: "Even volume" },
];

// ---------- Labels ----------

export function audioLabel(a) {
  const parts = [languageName(a.language), a.title, a.channel_layout ?? (a.channels ? `${a.channels} channels` : null), a.codec.toUpperCase()];
  return parts.filter(Boolean).join(", ");
}

export function subtitleLabel(t) {
  const parts = [languageName(t.language) ?? "Unknown language", t.title];
  if (t.forced) parts.push("forced");
  if (t.hearing_impaired) parts.push("SDH");
  if (!t.text) parts.push("drawn into the picture");
  return parts.filter(Boolean).join(", ");
}
