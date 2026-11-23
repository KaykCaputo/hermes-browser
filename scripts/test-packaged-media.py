"""Run under Xvfb with the path to an extracted Hermes Linux executable."""

from array import array
from contextlib import ExitStack
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


def run(*args):
    result = subprocess.run(args, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def has_test_tone(path):
    samples = array("h", path.read_bytes())
    if sys.byteorder != "little":
        samples.byteswap()
    # 440 Hz gives 44 rising zero crossings in a 100 ms window at 8 kHz.
    for start in range(0, len(samples) - 800, 800):
        segment = samples[start:start + 800]
        crossings = sum(a <= 0 < b for a, b in zip(segment, segment[1:]))
        if max(segment) > 256 and 40 <= crossings <= 48:
            return True
    return False


def has_visible_video(window):
    geometry = dict(line.split("=", 1) for line in
                    run("xdotool", "getwindowgeometry", "--shell", window).splitlines())
    # Inside the 320 x 180 test video, below the browser toolbar on Xvfb.
    x, y = int(geometry["X"]) + 16, int(geometry["Y"]) + 145
    pixels = subprocess.check_output([
        "ffmpeg", "-loglevel", "error", "-f", "x11grab", "-video_size", "256x80",
        "-i", f"{os.environ['DISPLAY']}+{x},{y}", "-frames:v", "1",
        "-pix_fmt", "rgb24", "-f", "rawvideo", "-threads", "1", "-",
    ])
    colored = sum(max(rgb) - min(rgb) > 60 for rgb in zip(*[iter(pixels)] * 3))
    return colored > 256 * 80 // 4


def main():
    executable = Path(sys.argv[1]).resolve(strict=True)
    with ExitStack() as stack:
        directory = stack.enter_context(tempfile.TemporaryDirectory())
        root = Path(directory)
        sink = f"hermes_media_test_{os.getpid()}"
        module = run("pactl", "load-module", "module-null-sink", f"sink_name={sink}")
        stack.callback(run, "pactl", "unload-module", module)
        recording = root / "audio.raw"
        output = stack.enter_context(recording.open("wb"))
        monitor = stack.enter_context(subprocess.Popen(
            ["parec", f"--device={sink}.monitor", "--raw", "--format=s16le",
             "--channels=1", "--rate=8000", "--latency-msec=50"], stdout=output))
        stack.callback(monitor.terminate)
        run("ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i",
            "testsrc2=size=160x90:rate=15", "-f", "lavfi", "-i",
            "sine=frequency=440", "-t", "4", "-c:v", "libvpx-vp9",
            "-c:a", "libopus", str(root / "video.webm"))
        (root / "index.html").write_text("""<!doctype html>
<title>Media test</title>
<style>body { margin:0 } button { position:fixed; inset:0; width:100%; height:100% }</style>
<video loop src="/video.webm" width="320" height="180"></video><button>Play</button>
<script>
const video = document.querySelector('video');
document.querySelector('button').onclick = event => {
  event.target.remove();
  document.title = 'Playing media test';
  video.play();
};
</script>""")
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=Hermes local test CA", "-addext", "basicConstraints=critical,CA:TRUE",
            "-keyout", str(root / "ca.key"), "-out", str(root / "ca.pem"))
        run("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
            "-keyout", str(root / "server.key"), "-out", str(root / "server.csr"))
        (root / "extensions").write_text("subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n")
        run("openssl", "x509", "-req", "-in", str(root / "server.csr"),
            "-CA", str(root / "ca.pem"), "-CAkey", str(root / "ca.key"), "-CAcreateserial",
            "-days", "1", "-extfile", str(root / "extensions"), "-out", str(root / "server.pem"))
        fixture = Path(__file__).resolve().with_name("media-test-server.mjs")
        server = subprocess.Popen(["node", fixture], cwd=root, stdout=subprocess.PIPE, text=True)
        try:
            port = int(server.stdout.readline())
            address = f"https://127.0.0.1:{port}/"
            env = dict(os.environ, PULSE_SINK=sink, XDG_STATE_HOME=directory, SSL_CERT_FILE=str(root / "ca.pem"))
            browser = subprocess.Popen([executable, address], cwd=root, env=env,
                                       start_new_session=True)
            try:
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline and browser.poll() is None:
                    windows = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", "Hermes Browser"],
                        text=True, capture_output=True).stdout.splitlines()
                    for window in windows:
                        title = run("xdotool", "getwindowname", window)
                        if title.startswith("Error loading page"):
                            raise RuntimeError("Browser failed to load the HTTP/2 fixture")
                        if title.startswith("Playing media test") and has_test_tone(recording):
                            if has_visible_video(window):
                                print("HTTP/2 navigation, visible VP9 frames and 440 Hz Opus audio passed")
                                return
                        if title.startswith("Media test"):
                            run("xdotool", "mousemove", "--window", window,
                                "300", "300", "click", "1")
                    time.sleep(0.4)
                raise RuntimeError("Packaged browser did not produce visible video and the expected audio signal")
            finally:
                if browser.poll() is None:
                    os.killpg(browser.pid, signal.SIGTERM)
                browser.wait(timeout=10)
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    main()
