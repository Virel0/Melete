// The music player's keyboard shortcuts, while Listen has the focus: on Listen's pages, or with
// focus in the music bar or panel. Keys typed into a field, or pressed on a button or link, are
// theirs, as with the video player's (player-keys.js). ? lists them; the command palette offers
// the same commands (musicCommands, for palette.js).
//
// The commands are the player's own (music-player.js, music-queue.js); liking asks Melete, as a
// track's Like does, and says what happened.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { audio, goNext, goPrevious, stopPlayer, toggle } from "/web/music-player.js";
import { current, cycleRepeat, repeating, shuffled, toggleShuffle } from "/web/music-queue.js";
import { LIKES_CLOSED } from "/web/music-access.js";

const REPEAT_SAID = { off: "Repeat off", all: "Repeating the queue", one: "Repeating this track" };

// Likes or unlikes the track playing, as its Like button does.
async function toggleLike() {
  const track = current();
  if (!track) return;
  try {
    const [now] = (await api(`/music/tracks?id=${track.id}&limit=1`)).items ?? [];
    if (!now) return announce(`${track.title} isn't in the library now.`);
    const want = !now.liked;
    await api(`/music/tracks/${track.id}/like`, { method: want ? "PUT" : "DELETE" });
    document.dispatchEvent(new CustomEvent("melete:like", { detail: { id: track.id, liked: want } }));
    announce(want ? `You like ${track.title}.` : `${track.title} is no longer liked.`);
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    announce(err.status === 403 ? LIKES_CLOSED : `Couldn't change it: ${err.message}`);
  }
}

const shuffle = () => {
  toggleShuffle();
  announce(shuffled() ? "Shuffle on" : "Shuffle off");
};
const repeat = () => {
  cycleRepeat();
  announce(REPEAT_SAID[repeating()]);
};

// Each shortcut: its keys as shown, the key it's pressed with, what it does, and the command.
const KEYS = [
  [["Space", "K"], [" ", "k"], "Play or pause", toggle],
  [["N"], ["n"], "Next track", goNext],
  [["P"], ["p"], "Previous track (or back to the start of this one)", goPrevious],
  [["S"], ["s"], "Shuffle on or off", shuffle],
  [["R"], ["r"], "Repeat: off, the queue, this track", repeat],
  [["L"], ["l"], "Like or unlike the track playing", toggleLike],
  [["?"], ["?"], "These shortcuts", () => openHelp()],
];

// The player's commands, for the palette, while something is queued.
export function musicCommands() {
  const track = current();
  if (!track) return [{ name: "Music keyboard shortcuts", kind: "Help", words: "keys listen player", run: () => openHelp() }];
  const playing = !audio.paused;
  const command = (name, words, run) => ({ name, kind: "Player", words, run });
  return [
    command(playing ? "Pause" : "Play", "music resume player", toggle),
    command("Next track", "music skip forward player", goNext),
    command("Previous track", "music back player", goPrevious),
    command(shuffled() ? "Shuffle off" : "Shuffle on", "music random player", shuffle),
    command({ off: "Repeat the queue", all: "Repeat this track", one: "Repeat off" }[repeating()], "music loop player", repeat),
    command(`Like or unlike ${track.title}`, "music heart favourite player", toggleLike),
    command("Stop and close the player", "music player", stopPlayer),
    { name: "Music keyboard shortcuts", kind: "Help", words: "keys listen player", run: () => openHelp() },
  ];
}

// Whether Listen has the focus: one of its pages is open, or focus is in the player.
function listening(target) {
  return location.hash.startsWith("#/music") || Boolean(target.closest?.(".music-bar, .music-panel"));
}

document.addEventListener("keydown", (event) => {
  if (event.ctrlKey || event.metaKey || event.altKey || event.defaultPrevented) return;
  const target = event.target;
  if (target.closest?.("input, select, textarea, button, a, summary, dialog, [contenteditable]")) return;
  if (!listening(target)) return;
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
  const found = KEYS.find(([, keys]) => keys.includes(key));
  // Only ? works with nothing queued: there's nothing to play, skip or like.
  if (!found || (!current() && key !== "?")) return;
  event.preventDefault();
  found[3]();
});

let help = null;

// The shortcuts, in a dialog: Escape or Close shuts it, and focus goes back where it was.
function openHelp() {
  help ??= makeHelp();
  if (!help.open) help.showModal();
}

function makeHelp() {
  const close = h("button", { type: "button", class: "quiet" }, "Close");
  const dialog = h(
    "dialog",
    { class: "palette music-keys", "aria-labelledby": "music-keys-head" },
    h("h2", { id: "music-keys-head" }, "Music keyboard shortcuts"),
    h("p", { class: "note" }, "On Listen's pages, or with the music player focused, while you're not typing."),
    h(
      "dl",
      { class: "music-keys-list" },
      KEYS.map(([shown, , what]) => h("div", {}, h("dt", {}, shown.map((k, i) => [i ? " or " : "", h("kbd", {}, k)])), h("dd", {}, what))),
    ),
    h("div", { class: "form-actions" }, close),
  );
  close.addEventListener("click", () => dialog.close());
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
  document.body.append(dialog);
  return dialog;
}
