//! A music file's tags and audio properties, read in-process with lofty. Files are opened for
//! reading only; nothing is ever written to them.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::FileType;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;

/// What one file says about itself. Text fields are trimmed, and empty ones are `None`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tags {
    /// Whether it has a tag with a title in it.
    pub tagged: bool,
    /// The track's title.
    pub title: Option<String>,
    /// The track's artist, as tagged, to show; several artist values are joined with "; ".
    pub artist: Option<String>,
    /// The track's artists, in order, when the tags list them apart: a multi-value artists tag
    /// (Vorbis and APE `ARTISTS`, ID3's `TXXX:ARTISTS`, MP4's `----:com.apple.iTunes:ARTISTS`),
    /// else several artist values (Vorbis `ARTIST` repeated, ID3v2.4 `TPE1` with several). Names
    /// are never split here: a list is only what the tags list.
    pub artists: Vec<String>,
    /// The album's artist, when tagged apart from the track's.
    pub album_artist: Option<String>,
    /// The album's title.
    pub album: Option<String>,
    /// Its number on its disc, from 1; none when untagged or 0.
    pub track_number: Option<i64>,
    /// Which disc it's on, from 1; none when untagged or 0.
    pub disc_number: Option<i64>,
    /// Its year, a four-digit one, from the date or year tag.
    pub year: Option<i64>,
    /// Its genres, split on `;` and with repeats (in any case) dropped.
    pub genres: Vec<String>,
    /// Whether it's tagged as part of a compilation.
    pub compilation: bool,
    /// Whether its tags carry a picture (served as the album's cover when there's no image file).
    pub has_picture: bool,
    /// Its length in seconds; none when the file doesn't say.
    pub duration_seconds: Option<f64>,
    /// Its codec, in lowercase (`flac`, `mp3`, `aac`, `alac`, `opus`…); none when unknown, as for
    /// MP4 audio lofty can't read.
    pub codec: Option<&'static str>,
    /// For gapless playback: samples to drop at the start and end.
    pub gapless: Option<super::gapless::Gapless>,
    /// Its bit rate in kbit/s.
    pub bitrate_kbps: Option<i64>,
    /// Its sample rate in Hz.
    pub sample_rate: Option<i64>,
    /// Its bit depth (16, 24), for formats that have one.
    pub bit_depth: Option<i64>,
    /// How many channels it has.
    pub channels: Option<i64>,
    /// Its ISRC.
    pub isrc: Option<String>,
    /// Its MusicBrainz recording id; only a valid MusicBrainz id is kept.
    pub mb_recording_id: Option<String>,
    /// Its MusicBrainz track id (the track on one release); only a valid MusicBrainz id is kept.
    pub mb_track_id: Option<String>,
    /// Its MusicBrainz release id; only a valid MusicBrainz id is kept.
    pub mb_release_id: Option<String>,
    /// Its MusicBrainz release group id; only a valid MusicBrainz id is kept.
    pub mb_release_group_id: Option<String>,
    /// Its main artist's MusicBrainz id: the first of several.
    pub mb_artist_id: Option<String>,
    /// Every MusicBrainz artist id its tags give, in order (taggers write one per artist).
    pub mb_artist_ids: Vec<String>,
    /// The album artist's MusicBrainz id: the first of several.
    pub mb_album_artist_id: Option<String>,
    /// Classical music: who wrote it, the work it's from, and which part of the work.
    pub composer: Option<String>,
    /// The work it's from (`Symphony No. 5 in C minor, Op. 67`).
    pub work: Option<String>,
    /// The part of the work it is (`Allegro con brio`).
    pub movement: Option<String>,
    /// That part's number in the work, from 1.
    pub movement_number: Option<i64>,
    /// How many parts the work has.
    pub movement_total: Option<i64>,
}

/// Reads a file's tags and properties, or says why it couldn't.
pub fn read(path: &Path) -> Result<Tags, String> {
    let file = open(path)?;
    let properties = file.properties();
    let duration = properties.duration().as_secs_f64();
    let mut tags = Tags {
        duration_seconds: (duration > 0.0).then_some(duration),
        codec: match file.file_type() {
            FileType::Mp4 => mp4_codec(path),
            other => codec(other, properties.bit_depth()),
        },
        bitrate_kbps: properties.audio_bitrate().map(i64::from),
        sample_rate: properties.sample_rate().map(i64::from),
        bit_depth: properties.bit_depth().map(i64::from),
        channels: properties.channels().map(i64::from),
        ..Tags::default()
    };
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        fill(&mut tags, tag);
    }
    tags.gapless = super::gapless::read(path, tags.codec);
    Ok(tags)
}

fn open(path: &Path) -> Result<lofty::file::TaggedFile, String> {
    let probe = Probe::open(path)
        .map_err(|err| err.to_string())?
        .options(ParseOptions::new())
        .guess_file_type()
        .map_err(|err| err.to_string())?;
    probe.read().map_err(|err| err.to_string())
}

/// The picture in a file's tags to show as its album's cover: the front cover, else the first
/// one; with its MIME type.
pub fn picture(path: &Path) -> Result<Option<(Vec<u8>, String)>, String> {
    let file = open(path)?;
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Ok(None);
    };
    let picture = tag
        .get_picture_type(lofty::picture::PictureType::CoverFront)
        .or_else(|| tag.pictures().first());
    Ok(picture.and_then(|p| {
        let mime = p.mime_type()?.as_str();
        mime.starts_with("image/")
            .then(|| (p.data().to_vec(), mime.to_owned()))
    }))
}

/// The lyrics in a file's tags (Vorbis `LYRICS`, ID3's `USLT`, MP4's `©lyr`), synced or not,
/// as written; none when no tag has any.
pub fn lyrics(path: &Path) -> Result<Option<String>, String> {
    let file = open(path)?;
    Ok(file.tags().iter().find_map(|tag| {
        [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
            .into_iter()
            .filter_map(|key| tag.get_string(key))
            .find(|text| !text.trim().is_empty())
            .map(str::to_owned)
    }))
}

fn fill(tags: &mut Tags, tag: &Tag) {
    let text = |value: Option<std::borrow::Cow<'_, str>>| clean(value.as_deref());
    let key = |item: ItemKey| clean(tag.get_string(item));
    tags.title = text(tag.title());
    tags.tagged = tags.title.is_some();
    fill_artists(tags, tag);
    tags.album = text(tag.album());
    tags.album_artist = key(ItemKey::AlbumArtist);
    tags.track_number = tag.track().map(i64::from).filter(|n| *n > 0);
    tags.disc_number = tag.disk().map(i64::from).filter(|n| *n > 0);
    tags.year = tag
        .date()
        .map(|d| i64::from(d.year))
        .or_else(|| key(ItemKey::Year).and_then(|y| year_of(&y)))
        .filter(|y| (1000..=9999).contains(y));
    let mut genres: Vec<String> = Vec::new();
    for value in tag.get_strings(ItemKey::Genre) {
        for genre in value.split([';', '\0']) {
            let genre = genre.trim();
            if !genre.is_empty() && !genres.iter().any(|g| g.eq_ignore_ascii_case(genre)) {
                genres.push(genre.to_owned());
            }
        }
    }
    tags.genres = genres;
    tags.compilation = key(ItemKey::FlagCompilation).is_some_and(|v| v == "1");
    tags.has_picture = !tag.pictures().is_empty();
    tags.isrc = key(ItemKey::Isrc);
    tags.mb_recording_id = key(ItemKey::MusicBrainzRecordingId).filter(|id| is_mbid(id));
    tags.mb_track_id = key(ItemKey::MusicBrainzTrackId).filter(|id| is_mbid(id));
    tags.mb_release_id = key(ItemKey::MusicBrainzReleaseId).filter(|id| is_mbid(id));
    tags.mb_release_group_id = key(ItemKey::MusicBrainzReleaseGroupId).filter(|id| is_mbid(id));
    // A track with several artists lists several ids; the first is the main one.
    tags.mb_artist_id = key(ItemKey::MusicBrainzArtistId).and_then(first_mbid);
    tags.mb_album_artist_id = key(ItemKey::MusicBrainzReleaseArtistId).and_then(first_mbid);
    // Several composers are one value with separators, or several values; the first is kept.
    tags.composer = tag
        .get_strings(ItemKey::Composer)
        .flat_map(|v| v.split([';', '\0']))
        .find_map(|v| clean(Some(v)));
    // In MP3s, Apple's players and Picard keep the work in TIT1 (otherwise the grouping) and
    // then move the grouping to GRP1.
    tags.work = key(ItemKey::Work)
        .or_else(|| key(ItemKey::AppleId3v2ContentGroup).and_then(|_| key(ItemKey::ContentGroup)));
    tags.movement = key(ItemKey::Movement);
    // MP3s keep both numbers in one frame, "3/4", which reads as both keys.
    let parse = |v: &str| v.trim().parse::<i64>().ok().filter(|n| *n > 0);
    let after_slash = |v: &str| v.split_once('/').and_then(|(_, total)| parse(total));
    let (number, total) = (key(ItemKey::MovementNumber), key(ItemKey::MovementTotal));
    tags.movement_number = number.as_deref().and_then(|v| parse(v.split('/').next()?));
    tags.movement_total = match total {
        Some(total) if Some(&total) != number.as_ref() => after_slash(&total).or(parse(&total)),
        _ => number.as_deref().and_then(after_slash),
    };
}

/// The artist to show, the artists as listed apart, and every MusicBrainz artist id.
fn fill_artists(tags: &mut Tags, tag: &Tag) {
    let artist_values = values(tag, ItemKey::TrackArtist);
    tags.artist = (!artist_values.is_empty()).then(|| artist_values.join("; "));
    let listed = values(tag, ItemKey::TrackArtists);
    tags.artists = if listed.is_empty() && artist_values.len() > 1 {
        artist_values
    } else {
        listed
    };
    let mut ids: Vec<String> = Vec::new();
    for value in tag.get_strings(ItemKey::MusicBrainzArtistId) {
        // Ids never hold these, so splitting on them is safe, unlike names.
        for id in value
            .split(['/', ';', ',', ' ', '\0'])
            .filter(|id| is_mbid(id))
        {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_owned());
            }
        }
    }
    tags.mb_artist_ids = ids;
}

/// Every value of a text item, in order: several items, or one holding values separated by
/// NULs. Trimmed, with empty ones and repeats (in any case) dropped.
fn values(tag: &Tag, key: ItemKey) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for value in tag.get_strings(key).flat_map(|v| v.split('\0')) {
        if let Some(value) = clean(Some(value))
            && !found.iter().any(|f| f.eq_ignore_ascii_case(&value))
        {
            found.push(value);
        }
    }
    found
}

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The first four-digit year in a date like "2001-05-01" or "2001".
pub fn year_of(text: &str) -> Option<i64> {
    text.get(..4)
        .filter(|y| y.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()
}

/// A MusicBrainz id: a lower-case UUID.
pub fn is_mbid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

fn first_mbid(ids: String) -> Option<String> {
    ids.split(['/', ';', ' '])
        .map(str::trim)
        .find(|id| is_mbid(id))
        .map(str::to_owned)
}

/// The codec of MP4 audio, as its sample entry says. Audio lofty can't read (E-AC-3 and the
/// like) has none, so ffprobe is asked about it (`music_spatial`) rather than it being
/// taken for AAC.
fn mp4_codec(path: &Path) -> Option<&'static str> {
    use lofty::file::AudioFile;
    use lofty::mp4::{Mp4Codec, Mp4File};
    let mut file = std::fs::File::open(path).ok()?;
    let mp4 = Mp4File::read_from(&mut file, ParseOptions::new().read_tags(false)).ok()?;
    Some(match mp4.properties().codec()? {
        Mp4Codec::AAC => "aac",
        Mp4Codec::ALAC => "alac",
        Mp4Codec::MP3 => "mp3",
        Mp4Codec::FLAC => "flac",
        _ => return None,
    })
}

fn codec(file_type: FileType, bit_depth: Option<u8>) -> Option<&'static str> {
    Some(match file_type {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Aac => "aac",
        // Read as an MP4 above; only ALAC (lossless) has a bit depth.
        FileType::Mp4 if bit_depth.is_some() => "alac",
        FileType::Mp4 => "aac",
        FileType::Opus => "opus",
        FileType::Vorbis => "vorbis",
        FileType::Speex => "speex",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::WavPack => "wavpack",
        FileType::Ape => "ape",
        FileType::Mpc => "musepack",
        _ => return None,
    })
}
