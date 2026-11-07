// The music player's sound: two decks, like a DJ's, so the next song can be ready and start
// the moment one ends, or fade in over it. One deck plays at a time as far as the pages know:
// `audio` stands for it, with the parts of an <audio> element they use (currentTime, duration,
// paused, volume, muted, play(), pause() and its events), and carries on from the other deck
// when they swap. The other deck is the spare, loaded with what comes next.
//
// Both decks go through Web Audio once anything plays: each its own gain (its fades and level),
// then the shared volume. Where Web Audio can't be had, each deck's own volume does it all.

const EVENTS = ["play", "playing", "pause", "ended", "timeupdate", "durationchange", "loadedmetadata", "seeking", "seeked", "waiting", "emptied", "error", "ratechange", "canplay", "progress", "stalled"];
// A fade is a curve of this many steps.
const STEPS = 64;

function makeDeck() {
  const el = new Audio();
  el.preload = "auto";
  // `level` is the track's own (level volume), `fade` where a fade has it; `track` the queue's
  // track it holds, and `converted` whether its file came converted.
  return { el, gain: null, level: 1, fade: 1, stepping: 0, track: null, converted: false };
}

const decks = [makeDeck(), makeDeck()];
let on = 0;
let context = null;
let master = null;
let volume = 1;
let muted = false;

export const active = () => decks[on];
export const spare = () => decks[1 - on];

class Output extends EventTarget {
  get currentTime() {
    return active().el.currentTime;
  }
  set currentTime(at) {
    active().el.currentTime = at;
  }
  get duration() {
    return active().el.duration;
  }
  get paused() {
    return active().el.paused;
  }
  get ended() {
    return active().el.ended;
  }
  get readyState() {
    return active().el.readyState;
  }
  get volume() {
    return volume;
  }
  set volume(value) {
    volume = Math.min(1, Math.max(0, Number(value) || 0));
    loudness();
  }
  get muted() {
    return muted;
  }
  set muted(value) {
    muted = Boolean(value);
    loudness();
  }
  play() {
    return start(active());
  }
  pause() {
    active().el.pause();
  }
}

export const audio = new Output();

// What the active deck says, the pages hear; the spare's own goings-on stay with it.
for (const deck of decks) {
  for (const type of EVENTS) {
    deck.el.addEventListener(type, () => deck === active() && audio.dispatchEvent(new Event(type)));
  }
}

// The Web Audio graph, made the first time something plays (a browser lets it start only then):
// each deck → its gain → the volume → the speakers. False where it can't be had.
function graph() {
  if (context !== null) return context;
  try {
    const Context = window.AudioContext ?? window.webkitAudioContext;
    const made = new Context();
    master = made.createGain();
    master.connect(made.destination);
    for (const deck of decks) {
      deck.gain = made.createGain();
      made.createMediaElementSource(deck.el).connect(deck.gain);
      deck.gain.connect(master);
    }
    context = made;
  } catch {
    context = false;
    master = null;
    for (const deck of decks) deck.gain = null;
  }
  loudness();
  for (const deck of decks) apply(deck);
  return context;
}

// Plays a deck, waking the graph first.
export function start(deck) {
  const made = graph();
  if (made && made.state !== "running") made.resume().catch(() => {});
  return deck.el.play();
}

function loudness() {
  if (master) {
    master.gain.value = muted ? 0 : volume;
    for (const deck of decks) {
      deck.el.volume = 1;
      deck.el.muted = false;
    }
  } else {
    for (const deck of decks) apply(deck);
  }
  audio.dispatchEvent(new Event("volumechange"));
}

// Sets a deck's gain from its level and fade, stopping any fade under way.
function apply(deck) {
  clearInterval(deck.stepping);
  deck.stepping = 0;
  const value = deck.level * deck.fade;
  if (deck.gain) {
    const now = context.currentTime;
    deck.gain.gain.cancelScheduledValues(now);
    deck.gain.gain.setValueAtTime(value, now);
  } else {
    deck.el.volume = Math.min(1, Math.max(0, volume * value));
    deck.el.muted = muted;
  }
}

// Puts a deck's fade at `value` (0 to 1) at once.
export function hold(deck, value) {
  deck.fade = value;
  apply(deck);
}

// A track's own level (level volume), as a factor of at most 1: glided to over a moment when
// the deck is sounding, so it never clicks.
export function setLevel(deck, value) {
  const level = Math.min(1, Math.max(0, Number.isFinite(value) ? value : 1));
  if (level === deck.level) return;
  deck.level = level;
  if (!deck.gain || deck.el.paused) return apply(deck);
  const now = context.currentTime;
  deck.gain.gain.cancelScheduledValues(now);
  deck.gain.gain.setTargetAtTime(deck.level * deck.fade, now, 0.05);
}

// Fades a deck along `curve` (progress 0 to 1 → level 0 to 1) over `seconds`.
export function fade(deck, curve, seconds) {
  apply(deck);
  deck.fade = curve(1);
  if (deck.gain) {
    const values = Float32Array.from({ length: STEPS }, (_, i) => deck.level * curve(i / (STEPS - 1)));
    deck.gain.gain.cancelScheduledValues(context.currentTime);
    deck.gain.gain.setValueCurveAtTime(values, context.currentTime, Math.max(seconds, 0.01));
    return;
  }
  // Without Web Audio, twenty steps a second on the element's own volume.
  const began = performance.now();
  deck.stepping = setInterval(() => {
    const progress = (performance.now() - began) / 1000 / Math.max(seconds, 0.01);
    deck.el.volume = Math.min(1, Math.max(0, volume * deck.level * curve(Math.min(progress, 1))));
    if (progress >= 1) clearInterval(deck.stepping);
  }, 50);
}

// Empties a deck: nothing loaded, at full level.
export function empty(deck) {
  deck.el.pause();
  deck.el.removeAttribute("src");
  deck.el.load();
  deck.converted = false;
  deck.track = null;
  deck.level = 1;
  hold(deck, 1);
}

// The spare deck becomes the one playing. The pages hear of it with refreshed().
export function swap() {
  on = 1 - on;
}

// Tells the pages what the deck now playing is at: its length, its time, playing or not.
export function refreshed() {
  for (const type of ["durationchange", "timeupdate", audio.paused ? "pause" : "play"]) audio.dispatchEvent(new Event(type));
}
