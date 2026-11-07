### Added

- Sign in with a code on the web sign-in page, for a browser with no keyboard to speak of (a
  TV's): the page shows a Quick Connect code, someone signed in elsewhere approves it under Your
  account → Quick Connect, and the page is signed in as them. `POST
  /auth/quick-connect/web-poll` is the apps' poll for the web UI: the same code lifetime,
  approval, single use and limits, and once approved it sets the same session cookie as a
  password sign-in, with the user alone in the body.
