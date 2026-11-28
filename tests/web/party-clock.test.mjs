// The watch party's rules for keeping players in step (src/web/party-clock.js), against a
// stand-in for a <video>. Run by tests/web_logic.rs: `node --test tests/web/party-clock.test.mjs`.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const source = readFileSync(new URL("../../src/web/party-clock.js", import.meta.url), "utf8");
const clockModule = await import(`data:text/javascript,${encodeURIComponent(source)}`);
const { correct, decide, partySeek, placeToSend, catchingUp, watchPlayer } = clockModule;

// Time as the tests set it: the browser's and the server's alike.
let now = 1_000_000;
Date.now = () => now;
const clock = { now: () => now };

// A playing <video> at `at` seconds, with `ahead` seconds buffered after it, watched as the
// players watch theirs from the start.
function video(at, { ahead = 30, paused = false } = {}) {
  const listeners = {};
  const v = {
    seeks: [],
    paused,
    playbackRate: 1,
    readyState: 4,
    seeking: false,
    duration: 3600,
    _at: at,
    get currentTime() {
      return this._at;
    },
    set currentTime(to) {
      this.seeks.push(to);
      this._at = to;
    },
    buffered: { length: 1, start: () => at - 10, end: () => at + ahead },
    addEventListener: (type, fn) => (listeners[type] ??= []).push(fn),
    fire: (type) => (listeners[type] ?? []).forEach((fn) => fn()),
    pause() {
      this.paused = true;
    },
  };
  watchPlayer(v);
  return v;
}

// A party playing since `now`, at `seconds` then.
const playing = (seconds, version = 1) => ({ playing: true, position_ms: seconds * 1000, position_at: now, version });

test("drift under 0.3 s is left alone", () => {
  for (const drift of [0, 0.1, -0.25, 0.29]) {
    assert.deepEqual(decide({ drift, buffering: false, rate: 1, sinceSeekMs: Infinity, ahead: 30, targetHeld: true }), { action: "none", rate: 1 });
  }
});

test("drift from 0.3 s to 4 s is played out by speed, never a seek", () => {
  const at = (drift, rate = 1) => decide({ drift, buffering: false, rate, sinceSeekMs: Infinity, ahead: 30, targetHeld: true });
  assert.deepEqual(at(0.5), { action: "rate", rate: 1.05 });
  assert.deepEqual(at(-0.5), { action: "rate", rate: 0.95 });
  assert.deepEqual(at(2), { action: "rate", rate: 1.1 });
  assert.deepEqual(at(-3.9), { action: "rate", rate: 0.9 });
  assert.deepEqual(at(3.99), { action: "rate", rate: 1.1 });
  // Once at a speed, it keeps it until within 0.1 s, then plays at 1×.
  assert.deepEqual(at(0.2, 1.05), { action: "rate", rate: 1.05 });
  assert.deepEqual(at(0.05, 1.05), { action: "none", rate: 1 });
  // Gone past the party: back to 1×, not the other way at once.
  assert.deepEqual(at(-0.2, 1.05), { action: "none", rate: 1 });
});

test("a player plays faster only with room in its buffer", () => {
  const low = (drift) => decide({ drift, buffering: false, rate: 1, sinceSeekMs: Infinity, ahead: 1, targetHeld: false });
  assert.deepEqual(low(1), { action: "none", rate: 1 });
  assert.deepEqual(low(-1), { action: "rate", rate: 0.95 });
});

test("past 4 s a player seeks, at most once in 10 s, forward or to a place it holds", () => {
  const at = (drift, sinceSeekMs, targetHeld = false) => decide({ drift, buffering: false, rate: 1, sinceSeekMs, ahead: 30, targetHeld });
  assert.equal(at(5, Infinity).action, "seek");
  assert.equal(at(5, 10_000).action, "seek");
  assert.deepEqual(at(5, 9_000), { action: "rate", rate: 1.1 });
  // Ahead: back to a place still buffered, else slower, unless it's far ahead.
  assert.equal(at(-5, Infinity, true).action, "seek");
  assert.deepEqual(at(-5, Infinity, false), { action: "rate", rate: 0.9 });
  assert.equal(at(-16, Infinity, false).action, "seek");
});

test("nothing is done while the player buffers", () => {
  for (const drift of [0.5, 3, 10, -10]) {
    assert.deepEqual(decide({ drift, buffering: true, rate: 1.1, sinceSeekMs: Infinity, ahead: 0, targetHeld: false }), { action: "none", rate: 1 });
  }
});

test("a player 2 s behind plays faster and isn't sought, on any stream", () => {
  const v = video(8);
  assert.equal(correct(v, playing(10), clock), "rate");
  assert.equal(v.playbackRate, 1.1);
  assert.deepEqual(v.seeks, []);
});

test("a buffering player is never sought, however far behind", () => {
  const v = video(0);
  v.fire("waiting");
  assert.equal(correct(v, playing(10), clock), "none");
  v.fire("playing");
  v.readyState = 2;
  assert.equal(correct(v, playing(10), clock), "none");
  v.seeking = true;
  v.readyState = 4;
  assert.equal(correct(v, playing(10), clock), "none");
  assert.deepEqual(v.seeks, []);
});

test("a catch-up seek leads by what the last one took, and comes at most once in 10 s", () => {
  const v = video(0);
  const p = playing(10);
  assert.equal(correct(v, p, clock), "seek");
  assert.deepEqual(v.seeks, [10]);
  assert.ok(catchingUp(v));
  v.fire("seeking");
  // It plays again 3 s later, now 3 s behind the party; slowed by nothing, it then falls back.
  now += 3000;
  v.fire("playing");
  assert.ok(!catchingUp(v));
  v._at = 10;
  now += 4000;
  // 7 s after the seek and 7 s behind: faster, not another seek.
  assert.equal(correct(v, p, clock), "rate");
  now += 3000;
  v._at = 11;
  assert.equal(correct(v, p, clock), "seek");
  // Party at 20 s; the seek lands 3 s past it, what the last one took.
  assert.equal(v.seeks.at(-1), 23);
});

test("someone's seek is followed at once, whatever the limits", () => {
  const v = video(10);
  correct(v, playing(10, 1), clock);
  now += 1000;
  v._at = 11;
  // Someone sought back 2 s: version 2 puts the party at 9 s.
  const moved = { playing: true, position_ms: 9000, position_at: now, version: 2 };
  assert.equal(correct(v, moved, clock), "seek");
  assert.equal(v.seeks.at(-1), 9);
  // Held for 30 s, then playing on from where it was held: not a move, so a player still
  // buffering isn't sought.
  const u = video(30);
  correct(u, playing(30, 1), clock);
  u.fire("waiting");
  correct(u, { playing: false, held: true, position_ms: 30_000, position_at: now, version: 2 }, clock);
  now += 30_000;
  assert.equal(correct(u, { playing: true, position_ms: 30_000, position_at: now - 1000, version: 3 }, clock), "none");
  assert.deepEqual(u.seeks, []);
  // The party merely held and played on from where it was: no seek.
  const w = video(20);
  correct(w, playing(20, 5), clock);
  now += 1000;
  w._at = 21.2;
  assert.equal(correct(w, { playing: true, position_ms: 21_000, position_at: now, version: 7 }, clock), "none");
  assert.deepEqual(w.seeks, []);
});

test("a paused party has every player where it paused, and a player not going yet joins it", () => {
  const v = video(12.5);
  assert.equal(correct(v, { playing: false, position_ms: 12_000, position_at: now, version: 1 }, clock), "seek");
  assert.deepEqual(v.seeks, [12]);
  // The player a party waited for, just behind: it stays, while the party's held and as it
  // plays on, and catches up by speed.
  const slow = video(11.2, { paused: true, ahead: 5 });
  assert.equal(correct(slow, { playing: false, held: true, position_ms: 12_000, position_at: now, version: 2 }, clock), "none");
  assert.equal(correct(slow, { playing: true, position_ms: 12_000, position_at: now + 400, version: 3 }, clock), "wait");
  assert.deepEqual(slow.seeks, []);
  slow.paused = false;
  slow.buffered = { length: 1, start: () => 11, end: () => 20 };
  now += 400;
  assert.equal(correct(slow, { playing: true, position_ms: 12_000, position_at: now, version: 3 }, clock), "rate");
  assert.equal(slow.playbackRate, 1.05);
  // Far behind, it's put there; and a party paused by hand has everyone where it paused.
  const far = video(5, { paused: true, ahead: 0.5 });
  assert.equal(correct(far, { playing: false, held: true, position_ms: 12_000, position_at: now, version: 2 }, clock), "seek");
  const near = video(11.2, { paused: true });
  assert.equal(correct(near, { playing: false, held: false, position_ms: 12_000, position_at: now, version: 2 }, clock), "seek");
  const late = video(0, { paused: true });
  assert.equal(correct(late, playing(30), clock), "seek");
  assert.deepEqual(late.seeks, [30]);
});

test("a seek of the party's own, or a step over a gap, isn't the viewer's", () => {
  const v = video(0);
  correct(v, playing(10), clock);
  v.fire("seeking");
  assert.ok(partySeek(v), "the party's own seek, however late its seeked comes");
  const w = video(40);
  w.fire("timeupdate");
  w.fire("seeking");
  w._at = 40.2;
  assert.ok(partySeek(w), "a player's step over a gap");
  w.fire("timeupdate");
  w.fire("seeking");
  w._at = 100;
  assert.ok(!partySeek(w), "the viewer's seek");
});

test("a pause is sent at the party's place, unless the viewer sought", () => {
  const p = playing(100);
  now += 2000;
  // 3 s behind as it played: its pause is sent at the party's place then.
  const behind = video(99);
  correct(behind, p, clock);
  now += 300;
  behind.paused = true;
  assert.equal(placeToSend(behind, p, clock, false), 102_000);
  assert.equal(placeToSend(behind, p, clock, true), 99_000);
  assert.equal(placeToSend(behind, { ...p, playing: false }, clock, false), 99_000);
  assert.equal(placeToSend(behind, null, clock, false), 99_000);
  // In step, its own place is the party's, sent as it is.
  const close = video(102.2);
  correct(close, p, clock);
  assert.equal(placeToSend(close, p, clock, false), 102_200);
});

test("each change in what sync does is noted for the playback report", () => {
  const noted = [];
  const note = (event, detail) => noted.push({ event, ...detail });
  const v = video(9);
  const p = playing(10);
  correct(v, p, clock, note);
  correct(v, p, clock, note);
  v._at = 10;
  correct(v, p, clock, note);
  v.fire("waiting");
  correct(v, p, clock, note);
  assert.deepEqual(
    noted.map(({ event, action, rate, buffering }) => [event, action, rate, buffering]),
    [
      ["sync", "rate", 1.05, false],
      ["sync", "none", 1, false],
      ["sync", "none", 1, true],
    ],
  );
  assert.deepEqual(Object.keys(noted[0]).sort(), ["action", "buffering", "drift_ms", "event", "rate", "reference"]);
  assert.equal(noted[0].drift_ms, 1000);
  assert.equal(noted[0].reference, 10);
});
