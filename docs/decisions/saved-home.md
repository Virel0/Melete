Decision: The web UI keeps the last `/home` answer in the browser's `localStorage`, under a key with the user's id, and draws it at once on Home before asking the server again; a failed refresh leaves it on screen. It's never drawn for another user, and every saved copy is removed when someone signs out and whenever the page finds no session (an ended session, or opening the page signed out), so it doesn't outlive the session that saved it.

Why: Home appears at once instead of after placeholders, and stays up when the server can't be reached, as Mneme's cached home does. `localStorage` rather than `sessionStorage` so a new tab or a reopened browser benefits too; what it holds is the user's own titles and progress, already shown in that browser, and clearing it with the session keeps it from lingering for whoever uses the browser next.

Date: 2026-11-06
