# Native Rust implementation over wrapping an external packager

drmpack implements DRM packaging natively in Rust rather than wrapping an external binary (e.g. Shaka Packager). A native implementation removes the external process dependency, gives full control over the encryption and muxing pipeline, and allows designing the API around the PackagingSession abstraction from day one.

## Considered options

- **Wrap Shaka Packager**: mature and battle-tested, but requires managing an external C++ binary, adds IPC overhead, and constrains the API to Shaka's CLI interface — no support for streaming OutputSink, per-quality-tier keys, or configurable encryption schemes without workarounds.
- **Fork Shaka Packager**: same C++ codebase, no Rust integration without FFI overhead.
