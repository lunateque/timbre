#!/bin/sh
set -eu
cd "$(dirname "$0")"

# The base tones use the same ffmpeg settings as edge/tags. That recipe also
# invokes the Monkey's Audio console encoder for the sole non-ffmpeg format.
source_dir=../../../../edge/tags/tests/fixtures
sh "$source_dir/generate.sh"
for fixture in "$source_dir"/tone.flac "$source_dir"/tone.opus \
    "$source_dir"/tone.wav "$source_dir"/tone.aiff; do
    cp "$fixture" "${fixture##*/}"
    test "$(wc -c < "${fixture##*/}")" -le 20480
done
ffmpeg -v error -y -f lavfi -i 'color=blue:s=32x32' -frames:v 1 -threads 1 embedded.png
ffmpeg -v error -y -f lavfi -i 'color=red:s=32x32' -frames:v 1 -threads 1 cover.jpg
