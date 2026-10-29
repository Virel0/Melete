//! How converted audio sounds: the stereo downmix, which keeps dialogue at the front, and the
//! optional compression and levelling.

use super::*;

/// How loud the centre channel, where speech is, goes into each side of a stereo downmix: at
/// the level of the front channels. ffmpeg's own downmix puts it 3 dB lower.
pub const CENTRE_LEVEL: f64 = 1.0;
/// The surround channels go in 3 dB lower than the front, ITU-R BS.775's level.
pub const SURROUND_LEVEL: f64 = 0.707;
/// The LFE channel goes in 10 dB lower, split between the two sides (13 dB lower in each);
/// ffmpeg's own downmix drops it.
pub const LFE_LEVEL: f64 = 0.316;

/// The downmix to stereo. The levels are scaled down together so the sum of all channels can't
/// pass full scale (`rematrix_maxval`): left alone, ffmpeg does that only for integer samples,
/// and the AAC encoder takes floats, so a loud scene would clip in the player.
fn downmix() -> String {
    format!(
        "aresample=ochl=stereo:clev={CENTRE_LEVEL}:slev={SURROUND_LEVEL}:lfe_mix_level={LFE_LEVEL}:rematrix_maxval=1"
    )
}

/// Mild compression for quiet rooms and small speakers: sound above −24 dB is reduced 3 to 1,
/// then everything is raised 6 dB, so speech comes up and explosions come down.
const DIALOGUE: &str = "acompressor=threshold=0.063:ratio=3:attack=10:release=250:makeup=2";

/// A steady loudness throughout, in one pass: −16 LUFS, peaks under −1.5 dBTP. loudnorm works
/// at 192 kHz, so the sound is brought back to 48 kHz after it.
const LEVELLED: &str = "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000";

/// What the audio sounds like in a conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioMode {
    /// As mixed; surround sound downmixed to stereo keeps the centre at the front's level.
    #[default]
    Normal,
    /// Mild compression on top, so speech is easier to follow at a low volume.
    Dialogue,
    /// A steady loudness, so quiet and loud parts (and files) play at about the same volume.
    Levelled,
}

/// The audio filters for a conversion: the downmix when `source` channels become 2, then the
/// mode's own. None when there's nothing to do.
pub fn audio_filter(mode: AudioMode, source: i64, out: i64) -> Option<String> {
    let mut filters: Vec<String> = Vec::new();
    if source > 2 && out == 2 {
        filters.push(downmix());
    }
    match mode {
        AudioMode::Normal => {}
        AudioMode::Dialogue => filters.push(DIALOGUE.into()),
        AudioMode::Levelled => filters.push(LEVELLED.into()),
    }
    (!filters.is_empty()).then(|| filters.join(","))
}

#[cfg(test)]
mod tests;
