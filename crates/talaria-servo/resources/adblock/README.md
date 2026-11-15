# Bundled adblock filters

Mercury builds its network blocker from the same validated filter snapshots distributed to Brave
clients. The files are pinned to commit `d4a2a0485d69067be2f6a412aeacdffef8291533` on the `lists`
branch of <https://github.com/brave/adblock-lists-mirror>.

The mirror maps each source URL to an MD5-named artifact. These snapshots are stored with readable
names here:

| Local file | Brave mirror artifact | SHA-256 | Licence |
| --- | --- | --- | --- |
| `ublock-filters.txt` | `ae9ffb3dd1624d8fa7e3fc3381ff11a4.txt` | `73d674807c88016fef58ba055381efea22585faab1a262a6d5fbab3a0bb62ccd` | GPL-3.0 |
| `easylist.txt` | `17ba74de8f13543dbff29e37b3ce125d.txt` | `ca554da776cd140d0a4803bc6a298c5d2251595e13ddbdcad888c347ee7ad1f8` | dual: GPL-3.0-or-later or CC-BY-SA-3.0 (or later) |
| `easyprivacy.txt` | `3017d28ef6b8789fc7064dbd848b8a7d.txt` | `6b4e1f32bfd3363c5dfd1afa5a0bb50277352843b7df469556d335e96f53e275` | dual: GPL-3.0-or-later or CC-BY-SA-3.0 (or later) |
| `brave-firstparty.txt` | `b85138f942539c8e8909d0c42779d662.txt` | `ea61ac33974c3f71712ad2862bb53da0d7de6638551fd3ba322b7f5f5f0f2135` | MPL-2.0 |
| `brave-specific.txt` | `e8b588fd6284a3127c4b195d102b75bb.txt` | `f2fa61e972136c51b052cf1e832480cbba22adf03c1210eaff5a654b8e0935a1` | MPL-2.0 |
| `brave-unbreak.txt` | `a1e25973d7f1fa978e3db67c3114dd5b.txt` | `63a1fdc6aeebbb93b8c15602b6180c85f9aadfddfab200b7c8f4e0cc8ee76159` | MPL-2.0 |

The files retain their upstream headers and remain subject to their respective upstream licenses.
They are data read by the content blocker, redistributed unmodified, and are not part of Mercury
Browser's source code. EasyList and EasyPrivacy are dual licensed by the EasyList authors under the
GNU GPL version 3 or later and CC BY-SA 3.0 Unported or later (<https://easylist.to/pages/licence.html>).
The uBlock filters are under the GNU GPL version 3 (the `LICENSE` file of uBlockOrigin/uAssets). The
three Brave lists carry no licence line; they are published from brave/adblock-lists, which is
MPL-2.0. The licence texts are in `LICENSES/` and `LICENSE` at the repository root; sources and
attribution are recorded in `THIRD-PARTY-NOTICES.md` under "Bundled data".

The build script compiles network rules only into an adblock-rust binary snapshot, so startup does
not need to parse the text lists or contact a remote server.
