//! Spatial Audio in the music library: Dolby Atmos (E-AC-3 with JOC, TrueHD with Atmos)
//! and multichannel files. The tag reader gives most files' channels itself; a file whose codec
//! it can't name (E-AC-3 in MP4, Matroska audio, raw E-AC-3, AC-3 or TrueHD) is asked about
//! with ffprobe in the background after each scan, one at a time, once per version of the
//! file. ffprobe names Atmos in the stream's profile ("Dolby Digital Plus + Dolby Atmos",
//! "Dolby TrueHD + Dolby Atmos").

use std::path::Path;

use sqlx::SqlitePool;

use crate::probe::{self, ProbeError};

/// What a track is, for players: Dolby Atmos, more than two channels, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spatial {
    /// Dolby Atmos.
    Atmos,
    /// More than two channels, without Atmos.
    Multichannel,
}

impl Spatial {
    /// What a track is, from whether ffprobe or its tags say Atmos and how many channels it has;
    /// none for mono or stereo.
    pub fn of(atmos: bool, channels: Option<i64>) -> Option<Spatial> {
        if atmos {
            Some(Spatial::Atmos)
        } else if channels.is_some_and(|c| c > 2) {
            Some(Spatial::Multichannel)
        } else {
            None
        }
    }

    /// Its name, as players are told it: `atmos` or `multichannel`.
    pub fn as_str(self) -> &'static str {
        match self {
            Spatial::Atmos => "atmos",
            Spatial::Multichannel => "multichannel",
        }
    }
}

/// Codecs that keep every bit, so a stereo copy can too (FLAC).
pub fn lossless(codec: &str) -> bool {
    matches!(
        codec,
        "flac" | "alac" | "wav" | "aiff" | "wavpack" | "ape" | "truehd" | "mlp"
    )
}

/// Whether ffprobe's profile for a stream says Dolby Atmos.
fn atmos(profile: Option<&str>) -> bool {
    profile.is_some_and(|p| p.to_ascii_lowercase().contains("atmos"))
}

/// What one round of asking ffprobe did, in tracks.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Tracks ffprobe answered for, and that were recorded.
    pub probed: usize,
    /// Tracks ffprobe couldn't make sense of; they aren't asked about again until the file changes.
    pub failed: usize,
    /// Tracks ffprobe timed out on or couldn't read; they're asked about again next round.
    pub retry_later: usize,
}

/// Asks ffprobe about every present track whose codec the tag reader couldn't name and that
/// hasn't been asked about in its present version. Stops before recording anything if ffprobe
/// can't run.
pub async fn probe_pending(pool: &SqlitePool, ffprobe: &Path) -> anyhow::Result<Report> {
    let pending: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT t.id, l.path, t.rel_path, t.size, t.modified_at
         FROM music_tracks t
         JOIN libraries l ON l.id = t.library_id
         LEFT JOIN music_track_probes p ON p.track_id = t.id
         WHERE t.missing_since IS NULL AND t.codec IS NULL
           AND (p.track_id IS NULL OR p.size <> t.size OR p.modified_at <> t.modified_at)
         ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = Report::default();
    for (id, root, rel_path, size, modified_at) in pending {
        let result = probe::probe(ffprobe, &Path::new(&root).join(&rel_path)).await;
        let (codec, profile, channels, layout, duration, error) = match result {
            Ok(info) => {
                report.probed += 1;
                let audio = info.audio.into_iter().next();
                let error = audio.is_none().then(|| "ffprobe found no audio".to_owned());
                match audio {
                    Some(a) => (
                        Some(a.codec),
                        a.profile,
                        a.channels,
                        a.channel_layout,
                        info.duration_seconds,
                        error,
                    ),
                    None => (None, None, None, None, info.duration_seconds, error),
                }
            }
            Err(ProbeError::Failed(reason)) => {
                tracing::warn!(rel_path, reason, "ffprobe rejected a music file");
                report.failed += 1;
                (None, None, None, None, None, Some(reason))
            }
            Err(err @ (ProbeError::TimedOut | ProbeError::Unreadable(_))) => {
                tracing::debug!(rel_path, error = %err, "music probe will be retried");
                report.retry_later += 1;
                continue;
            }
            Err(err @ ProbeError::Unavailable(_)) => anyhow::bail!("{err}"),
        };
        sqlx::query(
            "INSERT INTO music_track_probes (track_id, size, modified_at, codec, profile, channels,
                 channel_layout, atmos, duration_seconds, error, probed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (track_id) DO UPDATE SET size = excluded.size,
                 modified_at = excluded.modified_at, codec = excluded.codec,
                 profile = excluded.profile, channels = excluded.channels,
                 channel_layout = excluded.channel_layout, atmos = excluded.atmos,
                 duration_seconds = excluded.duration_seconds, error = excluded.error,
                 probed_at = excluded.probed_at",
        )
        .bind(id)
        .bind(size)
        .bind(modified_at)
        .bind(&codec)
        .bind(&profile)
        .bind(channels)
        .bind(&layout)
        .bind(atmos(profile.as_deref()))
        .bind(duration)
        .bind(&error)
        .bind(crate::db::unix_now())
        .execute(pool)
        .await?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atmos_is_read_from_the_profile_ffprobe_gives() {
        assert!(atmos(Some("Dolby Digital Plus + Dolby Atmos")));
        assert!(atmos(Some("Dolby TrueHD + Dolby Atmos")));
        assert!(!atmos(Some("LC")));
        assert!(!atmos(None));
        assert_eq!(Spatial::of(true, Some(2)), Some(Spatial::Atmos));
        assert_eq!(Spatial::of(false, Some(6)), Some(Spatial::Multichannel));
        assert_eq!(Spatial::of(false, Some(2)), None);
        assert_eq!(Spatial::of(false, None), None);
        assert!(lossless("truehd") && lossless("flac") && !lossless("eac3"));
    }

    /// A 5.1 E-AC-3 file and an MP4 holding one, made with ffmpeg.
    fn eac3(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1")
            .args(["-af", "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0"])
            .args(["-c:a", "eac3", "-b:a", "384k"])
            // As Apple's own files are: MP4, which the iPod muxer .m4a picks can't hold.
            .args(if name.ends_with(".m4a") {
                &["-f", "mp4"][..]
            } else {
                &[]
            })
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    #[test]
    fn the_tag_reader_leaves_e_ac_3_in_mp4_unnamed_rather_than_calling_it_aac() {
        let dir = tempfile::tempdir().unwrap();
        let tags = crate::library::tags::read(&eac3(dir.path(), "a.m4a")).unwrap();
        assert_eq!(tags.codec, None);
    }

    #[tokio::test]
    async fn files_the_tag_reader_cant_name_are_probed_once_per_version() {
        let pool = crate::db::open_in_memory().await;
        let dir = tempfile::tempdir().unwrap();
        eac3(dir.path(), "a.m4a");
        eac3(dir.path(), "b.ec3");
        let root = dir.path().to_str().unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(root)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (1, 1, 'k', 'A', 'B', '', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres, codec)
                 VALUES (1, 1, 'a.m4a', 1, 1, 1, 1, 1, 0, 'a', '[]', NULL),
                        (2, 1, 'b.ec3', 1, 1, 1, 1, 1, 0, 'b', '[]', NULL),
                        (3, 1, 'c.flac', 1, 1, 1, 1, 1, 1, 'c', '[]', 'flac'),
                        (4, 1, 'gone.ec3', 1, 1, 1, 1, 1, 0, 'd', '[]', NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let report = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        assert_eq!(
            report,
            Report {
                probed: 2,
                failed: 0,
                retry_later: 1
            },
            "a missing file is tried again later"
        );
        let rows: Vec<(i64, Option<String>, Option<i64>, bool)> = sqlx::query_as(
            "SELECT track_id, codec, channels, atmos FROM music_track_probes ORDER BY track_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                (1, Some("eac3".into()), Some(6), false),
                (2, Some("eac3".into()), Some(6), false),
            ]
        );
        assert_eq!(
            probe_pending(&pool, Path::new("ffprobe")).await.unwrap(),
            Report {
                probed: 0,
                failed: 0,
                retry_later: 1
            },
            "asked once per version"
        );
        sqlx::query("UPDATE music_tracks SET modified_at = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            probe_pending(&pool, Path::new("ffprobe"))
                .await
                .unwrap()
                .probed,
            1
        );
        sqlx::query("UPDATE music_tracks SET modified_at = 3 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let err = probe_pending(&pool, Path::new("/nonexistent/ffprobe"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ffprobe can't run"), "{err}");
    }
}
