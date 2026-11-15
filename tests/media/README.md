# Media test clips

Small synthetic clips used by `tests/pages/media.html`. They are original content generated from
FFmpeg's built-in test sources (`color`, `testsrc2`, `sine`); no third-party footage or audio is
included. Any FFmpeg with the encoders below can regenerate them (the copies here were made with
FFmpeg 9.0.2 on macOS; only the command-line tool was used, nothing links against it).

| File | Content | Size |
|---|---|---|
| `clip-h264-aac.mp4` | 4 s, 320×240, 25 fps, H.264 baseline + AAC-LC 440 Hz mono | about 89 KB |
| `clip-vp9-opus.webm` | 4 s, 320×240, 25 fps, VP9 + Opus 440 Hz mono | about 95 KB |
| `tone-opus.ogg` | 3 s, Opus 440 Hz mono | about 16 KB |
| `tone.mp3` | 3 s, MP3 440 Hz mono, 44.1 kHz | about 24 KB |
| `tone.wav` | 2 s, 16-bit PCM 441 Hz mono, 22.05 kHz | about 88 KB |

The video picture is a solid band across the top 80 rows — **red for the first 2 seconds, blue
for the last 2** — above FFmpeg's `testsrc2` pattern (moving gradient, timestamp and frame counter).
The test page samples the band at (160, 40) at two times to prove that frames decode and change.

## Commands

Run from this directory:

```sh
V='color=c=red:s=320x80:r=25:d=2[r];color=c=blue:s=320x80:r=25:d=2[b];[r][b]concat=n=2:v=1:a=0[band];testsrc2=s=320x160:r=25:d=4[pat];[band][pat]vstack,format=yuv420p[v]'

ffmpeg -hide_banner -loglevel error -y -filter_complex "$V" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=4" -map '[v]' -map 0:a \
  -c:v libx264 -profile:v baseline -level 3.0 -preset veryslow -crf 30 -g 25 \
  -c:a aac -b:a 48k -ac 1 -movflags +faststart \
  -map_metadata -1 -fflags +bitexact -flags:v +bitexact -flags:a +bitexact clip-h264-aac.mp4

ffmpeg -hide_banner -loglevel error -y -filter_complex "$V" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=4" -map '[v]' -map 0:a \
  -c:v libvpx-vp9 -b:v 0 -crf 45 -deadline good -row-mt 1 -g 25 \
  -c:a libopus -b:a 32k -ac 1 -map_metadata -1 -fflags +bitexact clip-vp9-opus.webm

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" \
  -c:a libopus -b:a 32k -ac 1 -map_metadata -1 -fflags +bitexact tone-opus.ogg

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=3" \
  -c:a libmp3lame -b:a 64k -ac 1 -map_metadata -1 -fflags +bitexact -write_xing 1 tone.mp3

ffmpeg -hide_banner -loglevel error -y -f lavfi -i "sine=frequency=441:sample_rate=22050:duration=2" \
  -c:a pcm_s16le -ac 1 -map_metadata -1 -fflags +bitexact tone.wav
```

## Running the checks

The page keeps every element muted with volume 0 and checks Web Audio with an
`OfflineAudioContext`, so it never makes a sound. Serve the repository over HTTP on the loopback
interface: from a `file://` URL the clips count as cross-origin, so drawing a video frame taints
the canvas and `XMLHttpRequest` cannot read the files for `decodeAudioData`.

```sh
python3 -m http.server --bind 127.0.0.1 8765 &
./target/release/mercury-browser --screenshot media.png --window-size 1100x1000 \
  --screenshot-delay 15000 "http://127.0.0.1:8765/tests/pages/media.html"
kill %1
```

Every check prints one line ending in `PASS` or `FAIL`, and a final `DONE` line.
