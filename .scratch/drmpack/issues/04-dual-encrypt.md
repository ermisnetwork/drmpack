# 04: Dual encryption pipeline (CENC + CBCS)

**What to build:** When a session is configured with `EncryptionScheme::Dual`, `PackagingSession` coordinates two parallel packaging branches from the same media input. One branch produces CENC-encrypted output using its own GPAC subprocess, and the other produces CBCS-encrypted output using a separate GPAC subprocess. The same input media is delivered to both branches, while their generated media and manifests remain isolated under `cenc/` and `cbcs/` subtrees in Ramdisk.

CENC and CBCS remain concrete encryption schemes; `Dual` is an orchestration mode and is not passed to GPAC as a concrete encryption scheme. Output is identified by `EncryptionScheme` rather than DRM-system-specific names.

A Dual session is treated as a single unit of work: failure of either packaging branch fails the `PackagingSession`, and shutdown attempts to close both branches.

This issue retains the existing key acquisition model. Production separation of CENC and CBCS ContentKeys/KIDs and Provider integration is intentionally deferred to follow-up work.

**Blocked by:** 02 (Low-Latency CMAF), 03 (Multi-DRM XML)

**Status:** done

* [x] `PackagingSessionConfig` accepts `EncryptionScheme::Dual`
* [x] A Dual session coordinates separate CENC and CBCS GPAC subprocesses
* [x] The same input media pushed to `PackagingSession` is delivered to both encryption branches
* [x] CENC output is isolated under the `cenc/` subtree in Ramdisk
* [x] CBCS output is isolated under the `cbcs/` subtree in Ramdisk
* [x] The CENC branch emits the required DASH manifest and CENC-encrypted CMAF segments
* [x] The CBCS branch emits the required HLS manifest and CBCS-encrypted CMAF segments
* [x] Dual manifests can be resolved by their concrete `EncryptionScheme`
* [x] `EncryptionScheme::Dual` is not treated as a concrete GPAC DRM encryption scheme
* [x] Both branches inherit the session's configured latency and packaging settings
* [x] Failure of either encryption branch is surfaced as a `PackagingSession` failure
* [x] Closing a Dual session attempts to close both packaging branches
* [x] Existing single-scheme CENC and CBCS session behavior remains compatible
* [x] Integration tests exercise Dual packaging through the public `PackagingSession` seam
* [x] Integration tests verify that CENC output signals the `cenc` protection scheme
* [x] Integration tests verify that CBCS output signals the `cbcs` protection scheme
* [x] Integration tests verify encrypted media output rather than only checking that output files exist
* [x] Production scheme-aware ContentKey/KID separation, CPIX, SPEKE v2, Axinom, and license integration remain out of scope

## Further Notes

* `EncryptionScheme::Dual` is primarily a compatibility strategy, not a requirement imposed by modern multi-DRM systems. It allows `drmpack` to provide a CENC/AES-CTR representation alongside a CBCS/AES-CBC representation when the target playback matrix requires both.
* CENC and CBCS are concrete ISO Common Encryption protection schemes, while `Dual` is a `drmpack` orchestration concept. GPAC should therefore always receive a concrete CENC or CBCS DRM configuration; `Dual` should never be translated into a GPAC encryption scheme.
* The Dual implementation intentionally uses two GPAC subprocesses. This reuses the existing `GpacProcess` execution seam and keeps each encryption pipeline isolated. A future implementation may optimize this into a single branched GPAC filter graph without changing the public `PackagingSession` contract.
* Output organization is based on encryption scheme (`cenc/` and `cbcs/`) rather than DRM-system names such as `widevine/` or `fairplay/`. Encryption scheme and DRM system are separate concepts: Widevine and PlayReady are not inherently limited to CENC, and CBCS is not inherently exclusive to FairPlay.
* CBCS with CMAF/fMP4 and HLS is the intended packaging path for FairPlay Streaming. Apple requires FairPlay-protected HLS to use `SAMPLE-AES` and the FairPlay key format `com.apple.streamingkeydelivery`. For Common Encryption video, Apple requires a 1:9 encrypted-to-clear block pattern. Complete FairPlay support therefore requires both correct CBCS media encryption and correct HLS DRM signaling. Issue 04 establishes the encryption/packaging topology; Provider-specific FairPlay signaling and license integration remain follow-up work.
* Modern DRM deployments do not necessarily require Dual output. Widevine supports CBCS on compatible clients, and PlayReady 4.0 introduced CBC support in addition to CENC/AES-CTR. Axinom supports Widevine, PlayReady, and FairPlay with multiple Common Encryption protection schemes, including CBCS. A production deployment with a sufficiently modern playback matrix may therefore choose CMAF + CBCS as a common packaging strategy instead of maintaining separate CENC and CBCS representations.
* CBCS support must not be assumed for every PlayReady client. PlayReady clients prior to version 4.0 do not support CBCS, and CBC support remains capability-dependent on newer clients. CENC therefore remains important when broad or legacy PlayReady compatibility is required.
* The reason for retaining Dual is consequently compatibility breadth: CENC provides the established AES-CTR path for Widevine/PlayReady environments, while CBCS provides the FairPlay-compatible path and can also serve modern DRM environments that support CBCS.
* The current Issue 04 key model is intentionally temporary. It is retained to keep this issue focused on proving dual packaging orchestration and must not be interpreted as the production ContentKey architecture.
* Production packaging must not encrypt the same content in CTR and CBC modes using the same `{KID, ContentKey}` pair. Microsoft explicitly warns against this for both functional and robustness reasons. A production Dual implementation therefore needs distinct key identity/material for its CENC and CBCS encryption contracts.
* The production key model should eventually be scheme-aware. Conceptually, key selection needs to account for the concrete `EncryptionScheme` in addition to existing track and quality usage so that CENC and CBCS branches can resolve appropriate ContentKeys independently.
* Scheme-aware key orchestration does not necessarily imply multiple Provider network requests. CPIX/SPEKE v2 supports multiple `ContentKey` entries in a single request/response document, and each key can identify its Common Encryption scheme through `commonEncryptionScheme`.
* CPIX/SPEKE v2 also provides `ContentKeyUsageRule` metadata describing which tracks a key protects. This aligns with the future direction of mapping Provider-supplied key material to encryption scheme, track type, quality tier, and eventually crypto period without embedding Provider-specific concepts into `PackagingSession`.
* Production scheme-aware ContentKey/KID separation, CPIX, SPEKE v2, Axinom integration, DRM-system-specific signaling, license acquisition, and license proxy behavior are deliberately deferred to follow-up Provider work.
* The follow-up Provider work should re-evaluate the production packaging policy before treating Dual as the default. The supported playback/device matrix should determine whether production uses CENC + CBCS Dual output or a simpler CBCS-oriented CMAF workflow.
* Existing single-scheme CENC and CBCS behavior should remain valid independently of the Dual implementation. Dual should be additive orchestration above those concrete packaging paths rather than a replacement for them.

## Detailed Specification

### Problem Statement

Media-server needs to deliver protected live media to broad device populations, including Apple/FairPlay clients that require CBCS and legacy Widevine or PlayReady clients that require CENC. A single concrete EncryptionScheme cannot reliably cover that playback matrix. Media-server currently has no PackagingSession mode that produces both compatible Representations from the same Segment stream while retaining the low-latency CMAF, Ramdisk, and GPAC subprocess lifecycle guarantees of the existing package path.

### Solution

Add `EncryptionScheme::Dual` as an opt-in PackagingSession orchestration mode. A Dual PackagingSession creates an isolated CENC Representation and CBCS Representation, each with its own GPAC subprocess, private DRM XML, Manifest set, and CMAF artifacts. It fans each input Segment or byte slice to both subprocesses concurrently and treats either Representation failing as failure of the whole PackagingSession.

Dual is not a concrete encryption scheme and is never passed to GPAC. CENC and CBCS remain the only concrete encryption schemes used to generate DRM XML and package media. The initial implementation deliberately reuses one temporary KeySet across both Representations; it establishes packaging topology only and is explicitly not a production-safe scheme-aware key architecture.

### User Stories

1. As a media-server developer, I want to opt into Dual packaging explicitly, so that the resource cost of producing two Representations is never accidental.
2. As a media-server developer, I want each PackagingSession to have one effective encryption mode, so that I cannot create ambiguous combinations of CENC, CBCS, and Dual.
3. As a media-server developer, I want the default encryption mode to remain CENC, so that existing integrations retain their current behavior unless they opt into another mode.
4. As a media-server developer, I want a Dual PackagingSession to package the same input media as CENC and CBCS concurrently, so that one ingestion session can serve the intended broad device matrix.
5. As a media-server developer, I want CENC and CBCS artifacts isolated in their own Ramdisk subtrees, so that manifests and CMAF artifacts from one Representation cannot overwrite the other.
6. As a media-server developer, I want single-scheme sessions to retain their existing flat Ramdisk layout, so that existing delivery integrations remain compatible.
7. As a media-server developer, I want to resolve a Manifest using a concrete EncryptionScheme and a Manifest format, so that delivery code never guesses directory or GPAC naming conventions.
8. As a media-server developer, I want HLS resolution to return the HLS master Manifest, so that an HLS player receives its protocol entrypoint rather than an implementation-specific variant Manifest.
9. As a media-server developer, I want invalid Manifest requests, including a request for Dual itself or an unavailable Representation, rejected clearly, so that I do not publish paths that a session will never create.
10. As a media-server developer, I want Manifest path resolution to work before a live Manifest is first written, so that route setup and delivery configuration do not race GPAC output creation.
11. As a media-server developer, I want the legacy single-scheme Manifest helpers to remain convenient but fail clearly for Dual, so that no helper silently selects the wrong Representation.
12. As a media-server developer, I want every Dual Representation to inherit latency mode, segment duration, chunk duration, live mode, and GPAC binary configuration, so that CENC and CBCS have equivalent packaging behavior apart from encryption.
13. As a media-server developer, I want input writes to wait for both Representations, so that a successful push means both GPAC subprocesses received the same ordered bytes and backpressure is explicit.
14. As a media-server developer, I want a failure in either Representation to fail-close the entire Dual PackagingSession, so that I never treat an incomplete compatibility set as successful output.
15. As a media-server developer, I want a failure to trigger best-effort shutdown of both subprocesses, so that a healthy branch is not left consuming resources after its counterpart fails.
16. As a media-server developer, I want all branch and cleanup failures reported in typed form, so that operations code can identify the affected Representation and lifecycle phase without parsing strings.
17. As a media-server developer, I want calls after an automatic failure to report the stored reason, so that later lifecycle code does not see a misleading generic closed-session error.
18. As a media-server developer, I want an unexpected GPAC exit, including exit code zero before explicit close, treated as a PackagingSession failure, so that a live session cannot silently stop accepting input.
19. As a media-server developer, I want `check_status()` to inspect every active Representation before it reports failure, so that simultaneous branch failures are observable together.
20. As a media-server developer, I want close to attempt every branch even when an earlier close fails, so that no child process is skipped because of error short-circuiting.
21. As a media-server developer, I want automatic cleanup to follow my configured cleanup policy after graceful and failed shutdown, so that Ramdisk usage and retained diagnostics are deliberate.
22. As a media-server developer, I want active sessions protected from manual output cleanup, so that an accidental cleanup call cannot corrupt an active live stream.
23. As a media-server developer, I want a configurable inactivity watchdog separate from the configurable GPAC finalization deadline, so that input silence and graceful output finalization can be tuned independently.
24. As a media-server developer, I want a Dual session to roll back partial creation atomically, so that I never receive a partially initialized PackagingSession.
25. As a media-server developer, I want Dual output roots to be exclusively owned and initially empty, so that rollback and cleanup cannot delete another session's artifacts.
26. As a media-server developer, I want default output roots to be unique per session, so that concurrent or restarted sessions with the same content ID do not collide.
27. As a media-server developer, I want raw ContentKey material kept outside the Ramdisk delivery output, so that manifests and CMAF artifacts can be served without accidentally exposing DRM XML.
28. As a media-server operator, I want private control-plane files restricted to the session owner where supported, so that temporary raw key material is not readable by unrelated local users.
29. As a media-server operator, I want private DRM XML removed after shutdown, rollback, or abandoned-session teardown, so that temporary key material is retained for the minimum practical lifetime.
30. As a media-server developer, I want to configure a private control-directory parent without learning the generated secret-bearing child path, so that delivery integrations do not couple themselves to internal key storage.
31. As a media-server developer, I want to know that dropping a PackagingSession is not graceful finalization, so that I explicitly await close before relying on final Manifests.
32. As a media-server developer, I want `key_set()` to remain available to the trusted in-process control plane, so that Dual's filesystem hardening does not falsely claim to isolate keys from the host application.
33. As a crate user, I want rustdoc to warn that Dual temporarily reuses key material across CENC and CBCS, so that I do not deploy the topology proof as production-safe key architecture.
34. As a maintainer, I want deterministic subprocess fault tests through the public PackagingSession seam, so that fan-out, rollback, and aggregate-error behavior are verified without relying on a real encoder failure.
35. As a maintainer, I want real GPAC end-to-end tests that inspect encrypted output for both Representations, so that successful file creation alone cannot mask broken Common Encryption packaging.
36. As a maintainer, I want DRM integration tests to fail in CI when GPAC or FFmpeg is missing, so that a skipped media test cannot make the required compatibility contract appear green.
37. As a maintainer, I want CI to use a reviewable pinned GPAC 26.07 build, so that external packaging behavior does not drift with the runner environment.
38. As a media-server developer, I want `drmpack` to leave HTTP publication timing to media-server, so that packaging remains separate from routing, authentication, and low-latency delivery policy.

### Implementation Decisions

- Model encryption selection as one scalar effective mode: Cenc, Cbcs, or Dual. The encryption-mode builder is a normal setter whose last call wins. Cenc remains the default.
- Treat Cenc and Cbcs as concrete EncryptionSchemes and Dual as an orchestration mode. Reject Dual at the GPAC DRM XML boundary instead of falling back to Cenc.
- Introduce a Manifest format domain type with DASH and HLS values. Resolve a canonical Manifest path from a concrete EncryptionScheme and Manifest format without checking filesystem readiness. The request must name a Representation that belongs to the PackagingSession; Dual itself is never a valid Representation selector.
- Keep the existing single-scheme delivery layout at the output root. A Dual session uses separate `cenc/` and `cbcs/` delivery subtrees, each containing GPAC-created DASH, HLS master, HLS variant, initialization, and CMAF artifacts.
- Treat the GPAC-created HLS master Manifest as the HLS public entrypoint. Variant/media Manifests remain internal output artifacts unless a later multi-Rendition feature introduces a deliberate variant API.
- Preserve single-scheme convenience helpers, but make their failure-capable contract explicit and reject their use for Dual sessions rather than silently choosing a Representation.
- Create two independent GPAC subprocesses for Dual rather than a combined GPAC filter graph. Each uses an independently generated concrete CENC or CBCS DRM XML configuration and receives identical packaging settings.
- Fetch one KeySet during Dual creation and generate both DRM XML configurations from it. This temporary shared-key topology is deliberately retained only until a later scheme-aware KeyProvider, KeyRequest, ContentKey, and KeyID design exists.
- Store DRM XML in a private session-scoped Control directory rather than in the Ramdisk delivery output. A caller-provided control directory is a parent directory; drmpack creates and owns an opaque unique child. Defaults prefer private tmpfs/Ramdisk storage and fall back to the system temporary directory.
- Keep DRM XML for the active lifetime of its GPAC subprocess because GPAC's cfile lifecycle must not be assumed to be eager-only. Delete control material best-effort after close, failure, rollback, and drop.
- On Unix, create control directories with owner-only permissions and DRM XML with owner-readable/writable permissions. On platforms without POSIX permissions, retain the private-storage separation without claiming equivalent permission enforcement.
- Require a Dual output root to be empty at creation. Track whether drmpack created the root: rollback removes a library-created root, while it preserves a previously existing caller-created empty root after removing only artifacts created by the failed attempt.
- Use unique default output roots to permit concurrent sessions for the same content ID without overwriting or deleting existing delivery artifacts.
- Fan out Segment and raw-byte input to CENC and CBCS concurrently. A push succeeds only after both writes and flushes succeed, so the slower Representation provides backpressure. Do not add internal queues for Dual.
- Make a Dual session one unit of work. On any creation, write, status, watchdog, or unexpected-exit failure, collect failures from both Representations where possible, fail-close the session, best-effort stop both subprocesses, and preserve the first observed terminal diagnostic for subsequent calls.
- Treat an early successful GPAC exit as unexpected while the caller has not begun explicit close. Explicit close is the only expected finalization path.
- Use shared internal lifecycle state so writes, status checks, close, and the inactivity watchdog can safely observe and establish the same terminal outcome while preserving the existing read-only status-check calling style.
- Define typed PackagingSession failures for every mode. They retain separate CENC and CBCS failure lists, output-cleanup and control-cleanup failures, and a BranchFailure containing concrete scheme, typed PackagingOperation, and the original DrmpackError. Do not permit Dual as a BranchFailure scheme.
- Return complete causal PackagingSession failures to the operation that detects them. Subsequent operations return a typed stored session failure containing the concrete Representation, lifecycle operation, and stable diagnostic.
- Always attempt all required branch shutdowns. `auto_cleanup` triggers output cleanup after explicit close or automatic fail-close even when branch finalization fails; retained output remains available for diagnostics when `auto_cleanup` is false.
- Restrict explicit output cleanup to terminal PackagingSession states. Dropping a live session never promises async GPAC finalization; it always attempts to remove private control material and removes delivery output only when `auto_cleanup` is enabled.
- Keep an inactivity watchdog duration separate from a configurable per-branch finalization timeout. Watchdog expiry is session-wide because input heartbeats are session-wide.
- Maintain the current trusted-control-plane boundary: `key_set()` remains public. Delivery-output isolation prevents accidental HTTP/static exposure of DRM XML but does not claim to hide ContentKeys from the in-process caller.
- Document on the public Dual API and in ADR-0006 that it is an experimental topology and must not be used as production-safe dual-scheme encryption until scheme-aware distinct KID and ContentKey selection is implemented.
- Keep Manifest generation separate from publication. drmpack exposes deterministic output locations; media-server owns HTTP routing, authentication, readiness gating, and the policy for observed artifacts after a failure.
- Pin GPAC 26.07 as the tested packaging contract. The DRM CI image builds the pinned GPAC source revision together with Rust and FFmpeg; a GPAC upgrade is a reviewable change accompanied by end-to-end validation.

### Testing Decisions

- Test the feature primarily at the public PackagingSession seam: create a session, push real fMP4 input, resolve Manifests, check lifecycle behavior, and close it. Do not make branch internals a consumer test seam.
- Retain focused unit coverage for mode validation, concrete-only GPAC XML generation, deterministic Manifest resolution, output ownership, cleanup restrictions, typed failure formatting/data, and private control-directory behavior.
- Add deterministic subprocess fault tests using a Rust fixture executable selected through the existing GPAC binary configuration seam. Each test owns a private scenario file that the fixture reads, avoiding process-global environment races while exercising real process, stdin, status, timeout, and teardown behavior.
- Verify Dual creation is all-or-nothing, including cleanup of resources created before the second Representation fails to initialize.
- Verify the same pushed input is received by both Representation subprocesses, preserves ordering, and applies slow-branch backpressure.
- Verify failures from write, unexpected exit, status check, watchdog shutdown, explicit close, output cleanup, and control cleanup are retained in the typed PackagingSession failure contract as applicable.
- Verify post-failure writes and status operations return the stored terminal reason and that close remains idempotent after automatic fail-close.
- Reuse existing real-GPAC CENC and CBCS integration-test patterns as prior art, but extend them through the public Dual PackagingSession seam.
- Capability-gate local real-media E2E tests with an explicit skip message when GPAC or FFmpeg is unavailable. In the DRM CI job, enable a required-media-test flag so missing tools fail the job rather than skip it.
- For both CENC and CBCS Representations, resolve and read the DASH Manifest and HLS master Manifest. Parse the HLS master to locate its variant Manifest, then verify HLS DRM signaling in the variant instead of incorrectly expecting key tags in the master.
- Verify DASH and HLS signaling matches the concrete Representation: CENC is signaled as CENC/SAMPLE-AES-CTR and CBCS as CBCS/SAMPLE-AES, with configured DRM metadata available from the temporary KeySet model.
- Inspect both initialization segments for the expected concrete protection scheme and KID, and inspect both CMAF media segments for sample-encryption auxiliary boxes and non-empty encrypted payload. Do not require decrypt-and-compare round trips in this issue.
- Add a GitHub Actions DRM workflow that runs on pull requests and pushes to main, builds the pinned media-test image, preflights GPAC and FFmpeg, and runs the required end-to-end suite.

### Out of Scope

- Production scheme-aware ContentKey and KeyID separation for CENC and CBCS.
- Changes to KeyProvider, KeyRequest, KeySet, CPIX, SPEKE v2, Axinom, or license-provider protocols needed for production dual-scheme key orchestration.
- FairPlay license acquisition, SPC/CKC handling, license proxy behavior, or Provider-specific DRM signaling policy.
- A device-specific playback matrix, device-capability detection, or automatic selection of a Representation by drmpack.
- HTTP serving, CDN publication, authentication, readiness gating, and artifact withdrawal policy; these remain media-server responsibilities.
- Replacing the two-subprocess topology with one branched GPAC filter graph.
- HLS variant-selection APIs tied to Rendition identity; that belongs with the multi-Rendition feature.
- Decrypt-and-compare interoperability tests, player-device certification, transcoding, key rotation, and VOD batch packaging.
- Retrofitting strict output-root ownership onto existing CENC-only or CBCS-only sessions.
- Removing the existing trusted in-process `key_set()` accessor or claiming cryptographic isolation from the hosting media-server process.

### Further Notes

- Dual exists for compatibility breadth, not because every modern DRM client requires two encryptions. Deployments with a validated modern playback matrix may later choose a single CBCS-oriented CMAF workflow.
- The temporary shared KeySet must not be mistaken for a production encryption policy. Reusing the same key identity and key material across CTR and CBC modes must be replaced before production Dual deployment.
- GPAC's HLS master Manifest is the public player entrypoint; its generated variant Manifest carries media-level key tags. Tests must distinguish those protocol roles.
- Existing ADR-0004 remains the rationale for persistent GPAC subprocesses, ADR-0005 remains the Ramdisk delivery rationale, and ADR-0006 records the deliberate Dual Representation trade-off.
- The single public integration seam for this feature is PackagingSession. Internal GPAC process details are verified indirectly through public lifecycle behavior and encrypted output.
