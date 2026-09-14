# HTTP Output Mode & v0.2.0 Baseline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the In-Process HTTP Output Mode (`EgressMode::HttpPush`) allowing GPAC to push segments directly to an in-memory loopback sink without disk staging, preceded by cleaning up 3 baseline code items.

**Architecture:** A pluggable `EgressMode` abstraction supporting `FileSystemStaging` (existing Page Cache baseline) and `HttpPush` (new in-memory loopback sink). In `HttpPush` mode, each `PackagingSession` binds a lightweight `hyper` HTTP/1.1 server on `127.0.0.1:0` with a session-scoped UUID token, receiving HTTP `PUT` requests from GPAC (`httpout:hmode=push`), verifying ISOBMFF boxes, and forwarding payloads directly into `mpsc::Sender<PackagedArtifact>` without any filesystem I/O or `notify` watchers.

**Tech Stack:** Rust 2021, Tokio, Hyper 1.x, Http-body-util, GPAC 26.07 (`httpout:hmode=push`), ISOBMFF box parsing.

---

## Tasks

### Task 1: Baseline Cleanups (Axinom Dead Code, LicenseProxy Fallback, QualityTier Allocation)

**Files:**
- Modify: `src/vendor/axinom/provider.rs:85-112`
- Modify: `src/license/proxy.rs:71-80`
- Modify: `src/types.rs:205-213`
- Test: `tests/axinom_provider_test.rs`
- Test: `tests/license_proxy_test.rs`

**Step 1: Write/update the failing test for QualityTier audio fallback**
In `src/types.rs` test module:
```rust
#[test]
fn test_quality_tier_audio_compat_fallback_match() {
    assert_eq!(QualityTier::audio().audio_compat_fallback(), Some(QualityTier::sd()));
    assert_eq!(QualityTier::sd().audio_compat_fallback(), Some(QualityTier::audio()));
    assert_eq!(QualityTier::hd().audio_compat_fallback(), None);
    assert_eq!(QualityTier::uhd_4k().audio_compat_fallback(), None);
    assert_eq!(QualityTier::new("custom").audio_compat_fallback(), None);
}
```

**Step 2: Run test to verify**
Run: `cargo test --lib types::tests::test_quality_tier_audio_compat_fallback_match`

**Step 3: Implement cleanups**
1. In `src/types.rs`:
```rust
pub fn audio_compat_fallback(&self) -> Option<Self> {
    match self.0.as_str() {
        "AUDIO" => Some(Self::sd()),
        "SD" => Some(Self::audio()),
        _ => None,
    }
}
```
2. In `src/license/proxy.rs`:
Remove redundant builder call inside `unwrap_or_else`:
```rust
pub fn new(config: impl Into<LicenseProxyConfig>) -> Self {
    let config = config.into();
    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()
        .expect("reqwest client builder with only timeout should never fail");
    Self::with_client(config, client)
}
```
3. In `src/vendor/axinom/provider.rs`:
Eliminate the dead `else` branch and awkward `pop()` calls for CENC + CBCS:
```rust
        if concrete_schemes.len() == 2 {
            let mut req_a = request.clone();
            req_a.encryption_schemes = vec![concrete_schemes[0]];
            let mut req_b = request.clone();
            req_b.encryption_schemes = vec![concrete_schemes[1]];

            let (set_a, set_b) = tokio::try_join!(
                self.fetch_single_scheme_keys(&req_a),
                self.fetch_single_scheme_keys(&req_b),
            )?;
            let mut combined_set = KeySet::new();
            for sub_set in [set_a, set_b] {
                for key in sub_set.all_keys() {
                    combined_set.insert_key(key.clone());
                }
                for pssh in sub_set.pssh {
                    combined_set.add_pssh(pssh);
                }
            }
            Ok(combined_set)
        } else {
            self.fetch_single_scheme_keys(request).await
        }
```

**Step 4: Run tests to verify they pass**
Run: `cargo test --lib` and `cargo test --test axinom_provider_test`

**Step 5: Commit**
```bash
git add src/types.rs src/license/proxy.rs src/vendor/axinom/provider.rs
git commit -m "refactor: clean up baseline code paths (LicenseProxy builder, Axinom dual join, QualityTier match)"
```

---

### Task 2: Domain Model & Configuration for `EgressMode`

**Files:**
- Modify: `src/types.rs`
- Modify: `src/session/mod.rs`
- Modify: `src/lib.rs`

**Step 1: Write failing test for `EgressMode` configuration**
In `src/session/mod.rs` tests:
```rust
#[test]
fn test_session_config_egress_mode() {
    let config = PackagingSessionConfig::new();
    assert_eq!(config.egress_mode, EgressMode::FileSystemStaging);

    let http_config = config.with_egress_mode(EgressMode::HttpPush);
    assert_eq!(http_config.egress_mode, EgressMode::HttpPush);
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --lib session::tests::test_session_config_egress_mode`
Expected: FAIL (EgressMode not found)

**Step 3: Implement `EgressMode`**
1. In `src/types.rs`:
```rust
/// Strategy for delivering packaged artifacts from GPAC into memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum EgressMode {
    /// Staged in temporary directory (OS /tmp backed by kernel Page Cache) and harvested by ArtifactHarvester (default).
    #[default]
    FileSystemStaging,
    /// Direct in-process HTTP loopback push from GPAC to RAM channel via httpout:hmode=push.
    HttpPush,
}
```
2. In `src/session/mod.rs`:
Add `pub egress_mode: EgressMode` to `PackagingSessionConfig`.
Add builder method:
```rust
pub fn with_egress_mode(mut self, mode: EgressMode) -> Self {
    self.egress_mode = mode;
    self
}
```
3. Export `EgressMode` in `src/lib.rs`.

**Step 4: Run test to verify it passes**
Run: `cargo test --lib session::tests::test_session_config_egress_mode`
Expected: PASS

**Step 5: Commit**
```bash
git add src/types.rs src/session/mod.rs src/lib.rs
git commit -m "feat(types): introduce EgressMode abstraction (FileSystemStaging vs HttpPush)"
```

---

### Task 3: In-Process HTTP Egress Server (`HttpEgressServer`)

**Files:**
- Modify: `Cargo.toml` (add `hyper` with server/http1 features and `http-body-util`)
- Create: `src/session/http_egress.rs`
- Modify: `src/session/mod.rs`

**Step 1: Write unit tests for `HttpEgressServer`**
In `src/session/http_egress.rs`:
```rust
#[tokio::test]
async fn test_http_egress_server_put_manifest_and_segment() {
    let (tx, mut rx) = mpsc::channel(16);
    let token = "test-auth-token-123";
    let server = HttpEgressServer::start(token.to_string(), tx).await.unwrap();
    let client = reqwest::Client::new();

    // 1. Valid manifest PUT
    let manifest_url = format!("{}/cbcs/video_720p.m3u8", server.endpoint_url());
    let res = client.put(&manifest_url).body("#EXTM3U\n#EXT-X-VERSION:7\n").send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let artifact = rx.recv().await.unwrap();
    assert_eq!(artifact.filename, "video_720p.m3u8");
    assert_eq!(artifact.scheme, EncryptionScheme::Cbcs);
    assert_eq!(artifact.kind, ArtifactKind::Manifest);

    // 2. Unauthorized PUT (wrong token)
    let bad_url = format!("http://{}/wrong-token/cbcs/live.mpd", server.local_addr());
    let res = client.put(&bad_url).body("<MPD/>").send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    server.shutdown().await;
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --lib session::http_egress::tests`
Expected: FAIL (module/type not found)

**Step 3: Implement `HttpEgressServer`**
In `Cargo.toml`:
```toml
hyper = { version = "1", features = ["server", "http1"] }
http-body-util = "0.1"
hyper-util = { version = "0.1", features = ["tokio", "server-auto"] }
```
In `src/session/http_egress.rs`:
- Bind `TcpListener` on `127.0.0.1:0`.
- Extract incoming URL: `/<token>/<scheme>/<filename>`.
- Authenticate `token`: if mismatch, return 403 Forbidden.
- Parse `scheme`: `cbcs` or `cenc`.
- Extract body using `http_body_util::BodyExt::collect(req.into_body()).await`.
- Validate binary ISOBMFF box (`ftyp`+`moov` for init, `moof`+`mdat` for media segment, or text for manifest).
- Construct `PackagedArtifact` and send through `mpsc::Sender<PackagedArtifact>`.
- Implement graceful cancellation via `tokio_util::sync::CancellationToken`.

**Step 4: Run test to verify it passes**
Run: `cargo test --lib session::http_egress`
Expected: PASS

**Step 5: Commit**
```bash
git add Cargo.toml src/session/http_egress.rs src/session/mod.rs
git commit -m "feat(session): implement HttpEgressServer loopback sink for zero-disk HTTP push"
```

---

### Task 4: GPAC Process Command Generation for `HttpPush`

**Files:**
- Modify: `src/gpac/process.rs`

**Step 1: Write failing test for GPAC arguments in HttpPush mode**
In `src/gpac/process.rs` tests:
```rust
#[test]
fn test_gpac_process_args_http_push() {
    let config = GpacProcessConfig::new(PathBuf::from("/tmp/test"), PathBuf::from("/tmp/drm.xml"))
        .with_http_egress_endpoint("http://127.0.0.1:45678/my-token/live.mpd");
    let args = config.build_args();
    assert!(args.iter().any(|a| a.contains("-o") || a.contains("http://127.0.0.1:45678/my-token/live.mpd:gpac:hmode=push")));
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --lib gpac::process::tests::test_gpac_process_args_http_push`

**Step 3: Implement GPAC argument generation for HTTP push**
In `src/gpac/process.rs`:
- Add `pub http_egress_endpoint: Option<String>` to `GpacProcessConfig`.
- In `build_args()`:
  - If `http_egress_endpoint` is `Some(endpoint)`:
    Dasher destination becomes:
    ```rust
    args.push("-o".into());
    args.push(format!("{endpoint}:gpac:hmode=push"));
    ```
    instead of writing to local filesystem path.

**Step 4: Run test to verify it passes**
Run: `cargo test --lib gpac::process::tests::test_gpac_process_args_http_push`
Expected: PASS

**Step 5: Commit**
```bash
git add src/gpac/process.rs
git commit -m "feat(gpac): support httpout:hmode=push destination filter in GpacProcessConfig"
```

---

### Task 5: Integrate `HttpPush` into `PackagingSession` Lifecycle & E2E Testing

**Files:**
- Modify: `src/session/mod.rs`
- Modify: `src/session/cluster.rs`
- Create: `tests/http_egress_e2e.rs`

**Step 1: Write E2E integration test**
In `tests/http_egress_e2e.rs`:
```rust
#[tokio::test]
async fn test_http_egress_live_cenc_e2e() {
    // Spin up a PackagingSession configured with EgressMode::HttpPush
    // Push test fMP4 media stream
    // Collect artifacts from session.take_output_receiver()
    // Verify manifests (.m3u8, .mpd) and segments (.m4s) received in RAM
    // Verify no files were created in /tmp
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test http_egress_e2e`

**Step 3: Wire `PackagingSession` with `HttpEgressServer`**
In `src/session/mod.rs`:
- If `config.egress_mode == EgressMode::HttpPush`:
  - Start `HttpEgressServer`.
  - Pass endpoint to `RepresentationCluster::spawn_gpac`.
  - Disable `ArtifactHarvester` background task.
- In `session.close()`:
  - Trigger `http_server.shutdown()`.

**Step 4: Run tests to verify they pass**
Run: `cargo test --test http_egress_e2e`
Expected: PASS

**Step 5: Commit**
```bash
git add src/session/mod.rs src/session/cluster.rs tests/http_egress_e2e.rs
git commit -m "feat(session): wire HttpPush into PackagingSession and add E2E verification test"
```

---

### Task 6: Documentation & Validation

**Files:**
- Modify: `ROADMAP.md`
- Modify: `CONTEXT.md`
- Modify: `README.md`

**Step 1: Update documentation**
- Check off `HttpPush` in `ROADMAP.md`.
- Document `EgressMode` in `CONTEXT.md` and `README.md` with example.

**Step 2: Verify full test suite**
Run: `cargo test -- --test-threads=1`
Run: `cargo clippy --all-targets -- -D warnings`
Run: `cargo fmt -- --check`

**Step 3: Commit**
```bash
git add ROADMAP.md CONTEXT.md README.md
git commit -m "docs: document EgressMode::HttpPush and update v0.2.0 roadmap"
```
