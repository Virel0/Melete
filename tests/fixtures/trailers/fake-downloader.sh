#!/bin/sh
# A stand-in for the trailer downloader, for tests: answers every job waiting in <jobs>/queue
# the way the real one does, with a one-second test pattern made by ffmpeg instead of a trailer
# from YouTube, so tests reach nothing outside. Job ids listed in GONE are answered as not on
# YouTube, those in FAIL as failed.
#
# The protocol: Melete writes queue/<id>.json ({"id", "video", "max_height"}). The downloader
# answers with done/<id>.json, written through a temporary name: {"id", "ok": true, "file":
# "<file in done/>", "height", "duration", "converted"}, or {"id", "ok": true, "unavailable":
# "<why>"}, or {"id", "ok": false, "error": "<why>"}; and removes the job from queue/.
set -eu
jobs="$1"
for job in "$jobs"/queue/*.json; do
    [ -e "$job" ] || continue
    id=$(basename "$job" .json)
    answer="$jobs/done/$id.json"
    case " ${GONE:-} " in
    *" $id "*)
        printf '{"id": "%s", "ok": true, "unavailable": "Video unavailable"}' "$id" >"$answer.tmp"
        ;;
    *)
        case " ${FAIL:-} " in
        *" $id "*)
            printf '{"id": "%s", "ok": false, "error": "yt-dlp ended with 1"}' "$id" >"$answer.tmp"
            ;;
        *)
            ffmpeg -nostdin -loglevel error -y -f lavfi -i testsrc2=s=64x36:r=24:d=1 \
                -pix_fmt yuv420p -movflags +faststart -f mp4 "$jobs/done/$id.mp4"
            printf '{"id": "%s", "ok": true, "file": "%s.mp4", "height": 2160, "duration": 1.0, "converted": true}' \
                "$id" "$id" >"$answer.tmp"
            ;;
        esac
        ;;
    esac
    mv "$answer.tmp" "$answer"
    rm -f "$job"
done
