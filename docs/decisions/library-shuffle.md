Decision: Songs and albums can be listed in a random order (`sort=random`), shuffled on the
server with a seed the first page answers, so later pages asked for with it continue the same
shuffle without repeats. A random order of songs leaves out the ones the listener marked Not
interested; the other orders still list them. Every order shows only what the listener may see.

Why: Shuffling the whole library needs more songs than a client holds, and a seeded order pages
like any list. Not interested is meant to keep a song out of shuffle, so the server's shuffle
honours it, while lists stay complete so the flag can be found and undone.

Date: 2026-11-07
