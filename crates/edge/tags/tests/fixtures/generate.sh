#!/bin/sh
set -eu
cd "$(dirname "$0")"

# Monkey's Audio console encoder (SDK 13.50), only needed to make tone.ape.
: "${MAC:=mac}"
render() {
    rate=$1
    shift
    ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=$rate:duration=1" \
        -map_metadata -1 -fflags +bitexact -flags:a +bitexact "$@"
}

render 44100 -c:a libmp3lame -q:a 4 -id3v2_version 0 -write_xing 0 tone.mp3
render 44100 -c:a libmp3lame -q:a 4 -id3v2_version 0 tone-xing.mp3
render 8000 -c:a flac tone.flac
render 44100 -c:a libvorbis -q:a 3 tone.ogg
render 48000 -c:a libopus -b:a 40k tone.opus
render 8000 -c:a alac tone.m4a
render 44100 -c:a aac -b:a 48k -movflags +faststart tone-aac.m4a
render 8000 -c:a pcm_s16le tone.wav
render 8000 -c:a pcm_s16be tone.aiff
render 8000 -c:a wavpack tone.wv
"$MAC" tone.wav tone.ape -c2000
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "color=c=green:s=18x12" \
    -frames:v 1 -update 1 -threads 1 cover.png

for fixture in tone.* tone-*.mp3 tone-*.m4a; do
    test "$(wc -c < "$fixture")" -le 20480
done
