# Bundled adblock filters

Hermes builds its network blocker from the same validated filter snapshots distributed to Brave
clients. The files are pinned to commit `06c4b9dd43a7132c7a563d6a6b02db6c9ec8f0b3` on the `lists`
branch of <https://github.com/brave/adblock-lists-mirror>.

The mirror maps each source URL to an MD5-named artifact. These snapshots are stored with readable
names here:

| Local file | Brave mirror artifact | SHA-256 |
| --- | --- | --- |
| `ublock-filters.txt` | `ae9ffb3dd1624d8fa7e3fc3381ff11a4.txt` | `59774b7433695ba1d437d4cc2d91b9cb3fc70e51a68e8a0ff3f4d2739c457c3c` |
| `easylist.txt` | `17ba74de8f13543dbff29e37b3ce125d.txt` | `5df869314c32eba1b96ba479bab8110664de8247fdcc568507a2e2312c13bd0b` |
| `easyprivacy.txt` | `3017d28ef6b8789fc7064dbd848b8a7d.txt` | `b5ed7cb4c908fd6d30f5091a706a7e70c4c211b7785cc01ee72f4f95fa8888e2` |
| `brave-firstparty.txt` | `b85138f942539c8e8909d0c42779d662.txt` | `7a134c2d9487fa9c8299f6267074a857bed588569edd570da5aa1d145714ebc6` |
| `brave-specific.txt` | `e8b588fd6284a3127c4b195d102b75bb.txt` | `ee24881b897217474f70eb6879c5b3df11d8bdf0ed6009945f361393481f3d8a` |
| `brave-unbreak.txt` | `a1e25973d7f1fa978e3db67c3114dd5b.txt` | `63a1fdc6aeebbb93b8c15602b6180c85f9aadfddfab200b7c8f4e0cc8ee76159` |

The files retain their upstream headers and remain subject to their respective upstream licenses.
The build script compiles network rules only into an adblock-rust binary snapshot, so startup does
not need to parse the text lists or contact a remote server.
