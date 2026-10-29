// What the app and the pages it shows share: which navigation is current, what the page on
// screen needs done when it goes, and the way back to the sign-in form. The app changes them
// and the pages read them, so no page imports the app back.

// Bumped on every navigation, so a slow answer for a page already left is dropped.
export let generation = 0;

// Starts a navigation (or signing out): answers still on their way for the last one are dropped.
export function nextGeneration() {
  generation += 1;
  return generation;
}

// What the page on screen needs done when it goes (the player's last progress report).
export const leaving = [];

let showSignIn = null;

// The app's sign-in form, which signIn shows; the app hands it over as it starts.
export function setSignIn(fn) {
  showSignIn = fn;
}

// Back to the sign-in form, with a message above it.
export function signIn(message) {
  showSignIn(message);
}
