# kithara-ui

- Do not work around the Vello clip/color-glyph defect by replacing clips, restricting glyph faces, or drawing color text outside its clip. Those change the rendering contract.
- Do not select a private fallback font when Fontique fails to resolve a system face. Fix or update the upstream resolution path.

[Contract rationale](https://github.com/zvuk/kithara/wiki/kithara-ui).
