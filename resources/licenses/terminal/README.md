# Terminal dependency notices

Newport uses the unmodified `alacritty_terminal` source at commit
`542147067dd016fc5f49120ef0ba605a98a4e3a7` from
https://github.com/alacritty/alacritty, licensed under Apache-2.0.
The adjacent `LICENSE-Alacritty` is copied from upstream.

The keyboard adapter follows the terminal protocols and the architecture of
Alacritty's host keyboard encoder. Newport's GPUI and SSH adapters are separate
from the emulator. No Zed source is included in this directory.

The parser dependency `vte` 0.15.0 (https://github.com/alacritty/vte),
`unicode-width` 0.2.2 (https://github.com/unicode-rs/unicode-width) and
`unicode-segmentation` 1.13.3 (https://github.com/unicode-rs/unicode-segmentation)
are distributed under their MIT option; their upstream notices are included
alongside Alacritty's.
