// Whether the signed-in user may see every music library (`sees_all_music` from /me). Radio,
// likes, playlists and the household's listening need every one, and the server refuses them to
// anyone limited to some music libraries; the Listen pages and the sidebar read this to leave
// them out and say so, rather than ask and be refused.

let all = true;

// Set once signed in, from the /me answer.
export function setMusicAccess(user) {
  all = user?.sees_all_music !== false;
}

export const seesAllMusic = () => all;

// Where radio, likes and playlists would be, for someone to whom they're closed.
export const SYNC_CLOSED =
  "Radio, likes and playlists aren't open to you, as you can see only some music libraries. An admin can change what you see.";

// What the like buttons say to someone to whom likes are closed.
export const LIKES_CLOSED = "Likes aren't open to you, as you can see only some music libraries. An admin can change what you see.";
