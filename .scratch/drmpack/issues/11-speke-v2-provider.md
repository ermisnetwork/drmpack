# 11: License Proxy Handlers

**What to build:** Async handler functions that `media-server` mounts on its HTTP routes to proxy client player license challenges to external DRM providers. One handler function per DRM system: `handle_widevine_license`, `handle_fairplay_license`, and `handle_playready_license`. Each handler receives the player's raw challenge bytes, adds required authentication tokens/headers, forwards to the provider license server URL, and returns the license response bytes.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] `handle_widevine_license(challenge_bytes, provider_config) -> Result<license_bytes>`
- [ ] `handle_fairplay_license(spc_bytes, provider_config) -> Result<ckc_bytes>`
- [ ] `handle_playready_license(challenge_bytes, provider_config) -> Result<license_bytes>`
- [ ] Integration tests proxying to mock DRM license server
