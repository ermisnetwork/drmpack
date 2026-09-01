# 13: License proxy handlers

**What to build:** Async handler functions that media-server mounts on its HTTP routes to proxy player license requests to the DRM provider. One handler per DRM system: Widevine, FairPlay, PlayReady. Each handler receives the player's license challenge, forwards it to the provider's license server URL, and returns the license response. Media-server is responsible for authentication and authorization middleware before the handler is called.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] `async fn handle_widevine_license(challenge_bytes, provider_config) -> Result<license_bytes>`: proxy Widevine license challenge to provider license URL
- [ ] `async fn handle_fairplay_license(spc_bytes, provider_config) -> Result<ckc_bytes>`: proxy FairPlay SPC to provider, return CKC
- [ ] `async fn handle_playready_license(challenge_xml, provider_config) -> Result<license_xml>`: proxy PlayReady license challenge
- [ ] Provider config contains license server URL and any required headers/credentials for the provider
- [ ] Handlers are pure functions (no state) — media-server passes config per call
- [ ] Test: mock license server, verify challenge is forwarded correctly and response is relayed back
- [ ] Test: verify error handling when provider is unreachable or returns error
