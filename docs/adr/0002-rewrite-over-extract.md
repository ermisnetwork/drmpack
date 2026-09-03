# Native Rust implementation over wrapping an external packager

**Status: Superseded by ADR-0004**

drmpack initially intended to implement DRM packaging natively in Rust rather than wrapping an external binary (e.g. Shaka Packager). A native implementation was thought to remove the external process dependency and give full control over the encryption and muxing pipeline.

## Reason for obsolescence

Native implementation poses extreme risks of silent CDM decryption failures (Widevine/FairPlay/PlayReady hardware decoders are strict black boxes) and massive development cost (parsing NALUs, subsample offsets, slice headers). Furthermore, Shaka Packager lacks LL-HLS support. See ADR-0004 for the decision to orchestrate GPAC as an isolated persistent subprocess.

## Considered options

- **Wrap Shaka Packager**: mature and battle-tested, but lacks LL-HLS (`EXT-X-PART`) support and CMAF chunking for very low latency.
- **Fork Shaka Packager**: same C++ codebase, no Rust integration without FFI overhead.
