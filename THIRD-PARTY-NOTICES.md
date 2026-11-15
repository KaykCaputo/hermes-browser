# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.

## Bundled data

The filter lists in `crates/talaria-servo/resources/adblock/` are data files read by the content
blocker, redistributed unmodified from the Brave mirror snapshot commit named in
`crates/talaria-servo/resources/adblock/README.md`; they are not part of Mercury Browser's source
code and keep their own licences.

- EasyList — The EasyList authors, https://easylist.to/ (source repository https://github.com/easylist/easylist) — local file `easylist.txt` — dual licensed under the GNU General Public License version 3 or (at your option) any later version, and Creative Commons Attribution-ShareAlike 3.0 Unported or (at your option) any later version; "The EasyList authors (https://easylist.to/)" should be attributed as the source. Stated at https://easylist.to/pages/licence.html, the page named by the list's `! Licence:` header. Texts: `LICENSES/GPL-3.0.txt`, `LICENSES/CC-BY-SA-3.0.txt`.
- EasyPrivacy — The EasyList authors, https://easylist.to/ (source repository https://github.com/easylist/easylist) — local file `easyprivacy.txt` — same licence and attribution as EasyList, stated at https://easylist.to/pages/licence.html, the page named by the list's `! Licence:` header. Texts: `LICENSES/GPL-3.0.txt`, `LICENSES/CC-BY-SA-3.0.txt`.
- uBlock filters — uBlock Origin uAssets, https://github.com/uBlockOrigin/uAssets (`filters/filters.txt`) — local file `ublock-filters.txt` — GNU General Public License version 3, the text in the repository's `LICENSE` file (read at commit `e140ffc75aee8c1af136f579126389d6defae5ed`), which the list's `! License:` header names. Text: `LICENSES/GPL-3.0.txt`.
- Brave Firstparty specific filters — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-firstparty.txt`) — local file `brave-firstparty.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
- Brave Specific — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-specific.txt`) — local file `brave-specific.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
- Brave unbreak list (its only line is `! Brave desktop filters`) — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-unbreak.txt`) — local file `brave-unbreak.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
