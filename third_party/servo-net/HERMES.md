# Servo network client

This is the MPL-2.0 servo-net 0.8.0 crate from crates.io. Hermes sets the
HTTP/2 response header list limit to 256 KiB in create_http_client.
YouTube watch pages can send security policies larger than Hyper's 16 KiB
default, which caused h2 to reset the stream before the player loaded.
On HTTPS requests to youtube.com, www.youtube.com, m.youtube.com and
www.youtube-nocookie.com, Hermes requests the tablet player with a Safari
user agent. This enables native GStreamer playback while Servo lacks the
Media Source Extensions required by the desktop player. Other hosts use
the normal Hermes user agent.

The DevTools integration skips response and security metadata construction
when no inspector channel is present. Recheck each change on Servo updates.
