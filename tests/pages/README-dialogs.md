# Page-request test pages

Pages that ask the browser something as soon as they have loaded, so that `--screenshot` shows the
browser's answer UI without any input. Serve `tests/` with `scripts/serve-tests.py` (loopback; its
`/private` route asks for HTTP Basic authentication and accepts only `alice` / `secret`). Loopback
is a secure context, which permission requests need.

| Page | Shows |
|---|---|
| `dialog-alert.html`, `dialog-confirm.html`, `dialog-prompt.html` | the dialog sheet |
| `dialog-spoof.html` | an alert whose message has direction overrides, a control character, CR LF and a long text |
| `dialog-twice.html` | two alerts waiting at once (the page and a frame on `localhost`), with "Don't allow this page to show more dialogs" |
| `permission.html` | `navigator.storage.persist()`: the permission prompt under the address bar |
| `permission-camera.html` | `navigator.permissions.request({name: "camera"})`: denied without a prompt |
| `http-auth.html` | a frame loading `/private`: the sign-in sheet (Servo asks only for navigations, not for `fetch()`) |

```sh
python3 scripts/serve-tests.py >serve.port &
sleep 1; port="$(head -n 1 serve.port)"
./target/release/mercury-browser --background --window-size 1100x700 --screenshot alert.png \
  --screenshot-delay 2000 "http://127.0.0.1:$port/pages/dialog-alert.html"
kill %1; rm serve.port
```
