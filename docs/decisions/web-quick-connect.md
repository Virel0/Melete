Decision: The web UI signs in with Quick Connect through its own poll, `/auth/quick-connect/web-poll`, which shares the apps' codes and every limit on them (five minutes, approval by someone signed in who looked the code up, one use, codes waiting per address, lookups per user) and, once approved, sets exactly the cookie a password sign-in sets, never putting the session in the body. It needs the `X-Melete-Web` header, as a change signed in by the cookie does. The page keeps the poll's secret in memory and sends it only in a request's body.

Why: A TV's browser has no keyboard to type a password with, and the apps' poll answers a bearer session the web UI can't use. One code store and one cookie keep the web from becoming a second, looser way in.

Date: 2026-11-07
