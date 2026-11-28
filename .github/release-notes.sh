#!/bin/bash
# The release page's notes, written by the release workflow: each image pinned by its full
# digest as a line ready to paste into .env, so pinning needs no pull-and-compare and no two
# images' digests can be crossed.
#
#   release-notes.sh melete <image> <version> <digest> <owner>   the notes for Melete's image
#   release-notes.sh stems|trailers <image> <version> <digest>   what each extra image adds
#
# <image> is the package without a tag (ghcr.io/owner/melete), <digest> sha256:<64 hex>.
set -euo pipefail

part=${1:?which image: melete, stems or trailers}
image=${2:?the image, without a tag}
version=${3:?the version}
digest=${4:?the digest}

if ! [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "not a full digest: $digest" >&2
    exit 1
fi

fence='```'
case "$part" in
melete)
    owner=${5:?the repository owner}
    pinned="$image:$version@$digest"
    cat <<EOF
Melete's image, pinned by its digest, for \`.env\`:

${fence}sh
MELETE_IMAGE=$pinned
${fence}

Check that it was built by this repository's release workflow:

${fence}
gh attestation verify oci://$pinned --owner $owner
${fence}

\`SHA256SUMS\` holds the SHA-256 of the image archive attached here, the same image as
an OCI archive (\`docker load\` or \`skopeo\` can read it). The changes are in CHANGELOG.md.
EOF
    ;;
stems | trailers)
    case "$part" in
    stems)
        what="The stem separator, for Auto DJ and sing-along (compose.stems.yaml)"
        variable=MELETE_STEMS_IMAGE
        ;;
    trailers)
        what="The trailer downloader (compose.trailers.yaml)"
        variable=MELETE_TRAILERS_IMAGE
        ;;
    esac
    printf "\n%s, for \`.env\`:\n\n%ssh\n%s=%s\n%s\n" \
        "$what" "$fence" "$variable" "$image:$version-$part@$digest" "$fence"
    ;;
*)
    echo "no such image: $part (melete, stems or trailers)" >&2
    exit 1
    ;;
esac
