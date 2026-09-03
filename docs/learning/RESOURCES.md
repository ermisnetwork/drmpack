# drmpack Resources

## Knowledge

- [ISO/IEC 23001-7: Common Encryption in ISO base media file format files (CENC)](https://www.iso.org/standard/68042.html)
  The core standard for CENC (`cenc` CTR mode, `cbcs` CBC pattern mode), PSSH box structure, and sample encryption auxiliary information (`senc`, `saiz`, `saio`, `tenc`, `sinf`). Use for: byte-exact box layouts and encryption rules.
- [ISO/IEC 14496-12: ISO base media file format (ISOBMFF)](https://www.iso.org/standard/68960.html)
  The foundational specification for fragmented MP4 (`ftyp`, `moov`, `moof`, `traf`, `trun`, `mdat`). Use for: box header parsing, `trun` sample offsets, and track definitions.
- [RFC 8216: HTTP Live Streaming (HLS)](https://datatracker.ietf.org/doc/html/rfc8216)
  The official HLS specification. Use for: `#EXT-X-KEY` syntax, `#EXT-X-MAP`, target duration calculation, and live vs VOD manifest formatting.

## Wisdom (Communities)

- [DASH-IF (DASH Industry Forum)](https://dashif.org/)
  Guidelines on Content Protection and interoperable multi-DRM packaging (CPIX, SPEKE v2).
- [Video Dev Slack (video-dev.org)](https://video-dev.org/)
  High-signal community of media streaming and packaging engineers.
