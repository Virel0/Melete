#!/bin/sh
# A stand-in for the motion cover renderer, for tests: answers every job waiting in
# <jobs>/queue the way the real renderer does, with a one-second test pattern made by ffmpeg
# instead of a loop rendered from the cover, so tests need no model. Album ids listed in SKIP
# are answered as skipped (mostly text), those in FAIL as failed.
#
# The protocol: Melete writes queue/<id>.cover (the cover's bytes) and then queue/<id>.json
# ({"id", "cover", "version", "loop"}). The renderer answers with done/<id>.json, written
# through a temporary name: {"id", "ok": true, "video": "<file in done/>", "details": {...}},
# or {"id", "ok": true, "skipped": "<why>"}, or {"id", "ok": false, "error": "<why>"}; and
# removes the job's files from queue/.
set -eu
jobs="$1"
for job in "$jobs"/queue/*.json; do
    [ -e "$job" ] || continue
    id=$(basename "$job" .json)
    answer="$jobs/done/$id.json"
    case " ${SKIP:-} " in
    *" $id "*)
        printf '{"id": "%s", "ok": true, "skipped": "mostly text"}' "$id" >"$answer.tmp"
        ;;
    *)
        case " ${FAIL:-} " in
        *" $id "*)
            printf '{"id": "%s", "ok": false, "error": "the cover could not be read"}' "$id" >"$answer.tmp"
            ;;
        *)
            ffmpeg -nostdin -loglevel error -y -f lavfi -i testsrc2=s=64x64:r=30:d=1 \
                -pix_fmt yuv420p -movflags +faststart -f mp4 "$jobs/done/$id.mp4"
            printf '{"id": "%s", "ok": true, "video": "%s.mp4", "details": {"side": 64}}' "$id" "$id" >"$answer.tmp"
            ;;
        esac
        ;;
    esac
    mv "$answer.tmp" "$answer"
    rm -f "$job" "$jobs/queue/$id.cover"
done
