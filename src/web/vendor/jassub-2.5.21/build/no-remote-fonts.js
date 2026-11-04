// Stands in for lfa-ponyfill, which fetches fonts from an outside host (Google Fonts).
// Melete never asks for remote fonts (queryFonts: false); this keeps that code out of the bundle.
export async function queryRemoteFonts () { return [] }
export default async function queryLocalFonts () { return [] }
