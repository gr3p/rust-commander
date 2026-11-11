#!/bin/sh
# Rebuilds tests/fixtures/audio with ffmpeg. The files are committed, so ffmpeg is only
# needed when a fixture changes. Every file is two seconds long, with a rising tone
# whose pitch tells a test where in the file it is. The surround files are shorter.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
out="$root/tests/fixtures/audio"
mkdir -p "$out"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
ff() { ffmpeg -loglevel error -y "$@"; }

chirp="aevalsrc=0.5*sin(2*PI*(300*t+400*t*t)):s=44100:d=2"
stereo="aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*660*t):s=44100:d=2:c=stereo"
surround="aevalsrc=0.4*sin(2*PI*220*t)|0.4*sin(2*PI*330*t)|0.4*sin(2*PI*440*t)|0.0|0.4*sin(2*PI*550*t)|0.4*sin(2*PI*660*t):s=16000:d=0.5:c=5.1"

ff -f lavfi -i "$chirp" -c:a pcm_s16le "$out/chirp.wav"
ff -f lavfi -i "$chirp" -c:a flac -compression_level 8 "$out/chirp.flac"
ff -f lavfi -i "$chirp" -c:a libmp3lame -b:a 48k "$out/chirp.mp3"
ff -f lavfi -i "$chirp" -c:a libvorbis -q:a 0 "$out/chirp.ogg"
ff -f lavfi -i "$chirp" -c:a libopus -b:a 24k "$out/chirp.opus"
ff -f lavfi -i "$chirp" -c:a aac -b:a 48k "$out/chirp.m4a"
ff -f lavfi -i "$stereo" -ar 16000 -c:a flac -compression_level 8 "$out/stereo.flac"
ff -f lavfi -i "$stereo" -c:a libopus -b:a 32k "$out/stereo.opus"
ff -f lavfi -i "$stereo" -c:a libmp3lame -b:a 64k "$out/stereo.mp3"
ff -f lavfi -i "$surround" -c:a flac "$out/surround.flac"
ff -f lavfi -i "$surround" -c:a libopus -b:a 64k -mapping_family 1 "$out/surround.opus"

# A 64x64 cover and tags, to check titles and artwork.
ff -f lavfi -i "color=c=0x2060c0:s=64x64:d=1" -frames:v 1 "$tmp/cover.jpg"
tags="-metadata title=Test_Song -metadata artist=Test_Artist -metadata album=Test_Album"
ff -f lavfi -i "$chirp" -i "$tmp/cover.jpg" -map 0:a -map 1:v -c:a libmp3lame -b:a 48k -c:v copy \
    -id3v2_version 3 -disposition:v attached_pic $tags "$out/tagged.mp3"
ff -f lavfi -i "$chirp" -i "$tmp/cover.jpg" -map 0:a -map 1:v -c:a flac -c:v copy \
    -disposition:v attached_pic $tags "$out/tagged.flac"
ff -f lavfi -i "$chirp" -i "$tmp/cover.jpg" -map 0:a -map 1:v -c:a aac -b:a 48k -c:v copy \
    -disposition:v attached_pic $tags "$out/tagged.m4a"
ff -f lavfi -i "$chirp" -c:a libopus -b:a 24k $tags "$out/tagged.opus"

ls -l "$out"
