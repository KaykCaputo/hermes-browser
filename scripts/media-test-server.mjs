// Loopback-only HTTP/2 fixture for the Linux archive playback test.
import { readFileSync } from "node:fs";
import { createSecureServer } from "node:http2";

const video = readFileSync("video.webm");
const server = createSecureServer({
    key: readFileSync("server.key"),
    cert: readFileSync("server.pem"),
});
server.on("stream", (stream, headers) => {
    // An older client can reset the stream when it rejects the header list.
    stream.on("error", () => {});
    if (headers[":path"] === "/video.webm") {
        stream.respond({ ":status": 200, "content-type": "video/webm", "content-length": video.length });
        stream.end(video);
    } else if (headers[":path"] === "/") {
        // Exercise the decoded header budget that YouTube watch pages exceeded.
        stream.respond({ ":status": 200, "content-type": "text/html", "x-policy-a": "a".repeat(12000), "x-policy-b": "b".repeat(12000) });
        stream.end(readFileSync("index.html"));
    } else {
        stream.respond({ ":status": 404 });
        stream.end();
    }
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
