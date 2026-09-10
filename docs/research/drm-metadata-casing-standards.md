# Research Report: DRM Metadata Casing Standards and Specifications Analysis

- **Document:** `docs/research/drm-metadata-casing-standards.md`
- **Date:** 2026-09-10
- **Status:** Completed & Validated Against Primary Standards

---

## 1. Executive Summary & Core Recommendation

### The Question
Should `scheme` (e.g. `"Cbcs"`, `"Cenc"`) and `track_type` (e.g. `"Video"`, `"Audio"`) in `drmpack` metadata (specifically `DrmStreamMetadata`, `DrmKeyEntry`, `EncryptionScheme`, and `TrackType`) be changed to lowercase (`"cbcs"`, `"cenc"`, `"video"`, `"audio"`) or kept as PascalCase (`"Cbcs"`, `"Video"`)?

### The Recommendation
**Change both `scheme` and `track_type` serialization to lowercase (`"cenc"`, `"cbcs"`, `"dual"`, `"video"`, `"audio"`, `"subtitle"`), while supporting legacy PascalCase and uppercase via Serde aliases (`#[serde(alias = "...")]`).**

This change is overwhelmingly supported by:
1. **International media standards:** ISO/IEC 23001-7 (FourCC is strictly 4-character lowercase ASCII), W3C Encrypted Media Extensions (EME `encryptionScheme` string is case-sensitive lowercase `"cenc"` / `"cbcs"`), DASH-IF CPIX 2.3, and AWS SPEKE v2.
2. **Downstream players & Web APIs:** Shaka Player, dash.js, video.js, and browser CDMs require lowercase strings in `MediaKeySystemConfiguration`.
3. **GPAC Filters:** GPAC `cecrypt` XML strictly expects `type="cenc"` or `type="cbcs"`.
4. **Internal Codebase Consistency:** `impl fmt::Display` for both `EncryptionScheme` and `TrackType` in `src/types.rs` already outputs lowercase strings (`"cenc"`, `"cbcs"`, `"video"`, `"audio"`). The current PascalCase serialization was an unintentional artifact of `#[derive(Serialize, Deserialize)]` defaulting to Rust variant names.

By applying `#[serde(rename_all = "lowercase")]` alongside `#[serde(alias = "Cenc")]` etc., the migration is **100% backwards-compatible**: existing database records in PostgreSQL/Redis and legacy clients continue to deserialize without error.

---

## 2. Primary Source Investigations

### 2.1 ISO/IEC 23001-7 & ISO/IEC 14496-12 (ISO-BMFF Common Encryption)

- **Standards:**
  - **ISO/IEC 23001-7:2016 / 2023** (*Information technology — MPEG systems technologies — Part 7: Common encryption in ISO base media file format files*)
  - **ISO/IEC 14496-12:2022** (*Information technology — Coding of audio-visual objects — Part 12: ISO base media file format*)

- **Scheme Type (FourCC):**
  - Section 4 (*Protection System Overview*) and Section 5 (*Protection Schemes*) define the protection scheme FourCC placed in the `schm` (*Scheme Type Box*) inside `sinf` (*Protection Scheme Information Box*).
  - Common Encryption schemes are registered as 4-character lowercase ASCII codes:
    1. `'cenc'`: AES-CTR mode with full sample and video NAL subsample encryption (no pattern).
    2. `'cbcs'`: AES-CBC mode with 10% pattern encryption (`crypt_byte_block=1`, `skip_byte_block=9` for video; whole-block `0:0` for audio).
    3. `'cbc1'`: AES-CBC mode with full sample and video NAL subsample encryption (no pattern).
    4. `'cens'`: AES-CTR mode with pattern encryption.
  - In ISO-BMFF binary boxes, a FourCC is a 32-bit unsigned integer corresponding to ASCII byte values:
    - `'cenc'` = `0x63656E63`
    - `'cbcs'` = `0x63626373`
  - PascalCase identifiers like `'Cenc'` (`0x43656E63`) or `'Cbcs'` (`0x43626373`) are non-compliant under ISO/IEC 23001-7. Demuxers and CDMs (Apple AVFoundation, Google Widevine CDM, Microsoft PlayReady) reject them as unrecognized scheme types.

- **Track Types (Handler Reference):**
  - In ISO/IEC 14496-12, elementary stream types are signaled in the `hdlr` (*Handler Reference Box*) using lowercase 4CC codes:
    - `'vide'` for Video track
    - `'soun'` for Sound/Audio track
    - `'subt'` for Subtitle track
    - `'text'` for Timed text track
    - `'meta'` for Metadata track

---

### 2.2 W3C Encrypted Media Extensions (EME) Specification

- **Standard:** W3C Recommendation (*Encrypted Media Extensions*, [W3C EME](https://www.w3.org/TR/encrypted-media/)).

- **`encryptionScheme` in EME:**
  - Section 3.4 (*MediaKeySystemMediaCapability dictionary*) defines:
    ```webidl
    dictionary MediaKeySystemMediaCapability {
      DOMString contentType = "";
      DOMString? encryptionScheme = null;
      DOMString robustness = "";
    };
    ```
  - Section 3.4.1 (*Dictionary MediaKeySystemMediaCapability Members*) explicitly enumerates well-known values:
    - **`cenc`**: The 'cenc' mode, defined in [CENC], section 4.2a.
    - **`cbcs`**: The 'cbcs' mode, defined in [CENC], section 4.2d.
    - **`cbcs-1-9`**: The same as 'cbcs' mode, with encrypt:skip pattern of 1:9.

- **Strict Case Sensitivity in Browsers:**
  - Section 3.2.2.3 (*Get Supported Capabilities for Audio/Video Type*) specifies that string comparison in EME is **case-sensitive**.
  - Calling `navigator.requestMediaKeySystemAccess()` with `{ encryptionScheme: "Cenc" }` or `{ encryptionScheme: "Cbcs" }` causes the browser's CDM to reject the capability and throw `NotSupportedError`.
  - Production players (Shaka Player, dash.js) configure EME using lowercase `"cenc"` / `"cbcs"`.

---

### 2.3 DASH-IF CPIX 2.3 Specification & AWS SPEKE v2

- **Standards:**
  - **DASH-IF CPIX 2.3** (*Content Protection Information Exchange Format*, [DASH-IF CPIX](https://dashif.org/docs/CPIX2.3/Cpix.html))
  - **AWS SPEKE v2.0** (*Secure Packager and Encoder Key Exchange API Specification*, [AWS SPEKE](https://docs.aws.amazon.com/speke/latest/documentation/))

- **`commonEncryptionScheme` in CPIX 2.3:**
  - In Section 5.2.5 (*ContentKey Element*):
    - Defined as attribute `commonEncryptionScheme (O, xs:string with length=4)`.
    - Normative requirement: *"When present, the value shall be a 4-character protection scheme name as defined by [MPEGCENC]."*
    - Therefore, the attribute value is strictly lowercase `"cenc"` or `"cbcs"`.

- **Track Signaling in CPIX 2.3 & AWS SPEKE v2:**
  - Modeled using XML filter elements under `<cpix:ContentKeyUsageRule>`: `<cpix:VideoFilter>` and `<cpix:AudioFilter>`.
  - In AWS SPEKE v2 constraints, `commonEncryptionScheme` is mandatory and strictly lowercase `"cenc"` or `"cbcs"`.

---

### 2.4 Axinom DRM API & Axinom Entitlement JWT

- **Standard:** Axinom DRM Services Documentation ([Axinom DRM](https://docs.axinom.com/services/drm/)).

- **Axinom Entitlement JWT Message (v2):**
  - Payload structure follows standard JSON `snake_case` / `lowercase`:
    ```json
    {
      "version": 1,
      "com_key_id": "12196546-a312-4a4e-ab65-c19bccce1adf",
      "message": {
        "version": 2,
        "type": "entitlement_message",
        "content_keys_source": {
          "inline": [
            {
              "id": "c9a0d00c-cd2b-4509-aaa8-95b5d68a7382",
              "iv": "EREREREREREREREREREREQ=="
            }
          ]
        }
      }
    }
    ```
  - **Finding:** The Axinom Entitlement JWT **does NOT contain** `scheme` or `track_type` fields! Entitlements are strictly keyed by `id` (Key ID GUID) and optional `iv`.
  - In Axinom Key Service CPIX requests, `commonEncryptionScheme` is strictly lowercase `"cbcs"` or `"cenc"`.

---

### 2.5 GPAC cecrypt / dasher

- **Source:** GPAC Filters (`cecrypt.c`, `dasher.c`).
- GPAC DRM XML root element requires `type="cenc"` or `type="cbcs"`.
- In `drmpack` (`src/gpac/xml.rs:92-103`), the generator already hardcodes lowercase strings for GPAC:
  ```rust
  let scheme_str = match config.scheme {
      EncryptionScheme::Cenc => "cenc",
      EncryptionScheme::Cbcs => "cbcs",
      ...
  };
  ```

---

## 3. Codebase Analysis in `drmpack`

### 3.1 The Internal Inconsistency in `src/types.rs`
In `src/types.rs`:
```rust
impl fmt::Display for EncryptionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncryptionScheme::Cenc => write!(f, "cenc"),
            EncryptionScheme::Cbcs => write!(f, "cbcs"),
            EncryptionScheme::Dual => write!(f, "dual"),
        }
    }
}

impl fmt::Display for TrackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrackType::Video => write!(f, "video"),
            TrackType::Audio => write!(f, "audio"),
            TrackType::Subtitle => write!(f, "subtitle"),
        }
    }
}
```
`Display` yields `"cenc"` / `"video"`, but Serde without attributes defaulted to PascalCase `"Cenc"` / `"Video"`.

### 3.2 Impact on Downstream Services
In `examples/09_axum_playback_server.rs:135`, the server was already forced to call `.to_lowercase()` to handle query parameters and player requests:
```rust
let scheme = query.scheme.to_lowercase();
```

---

## 4. Comparison Table

| Standard / Component | PascalCase (`"Cbcs"`, `"Video"`) | Lowercase (`"cbcs"`, `"video"`) |
| :--- | :--- | :--- |
| **ISO/IEC 23001-7 (CENC 4CC)** | ❌ Non-compliant (4CC is strictly `'cenc'`, `'cbcs'`) | ✅ Fully compliant |
| **W3C EME Specification** | ❌ Fails `requestMediaKeySystemAccess` | ✅ Native browser compatibility |
| **DASH-IF CPIX 2.3** | ❌ Mismatches `commonEncryptionScheme` | ✅ Matches 4-character scheme name |
| **GPAC `cecrypt`** | ❌ Incompatible with `type="cenc"` | ✅ Directly compatible |
| **Rust `fmt::Display`** | ❌ Inconsistent with `serde` | ✅ 100% consistent |
| **Backwards Compatibility** | ⚠️ Only reads PascalCase | ✅ Reads both old and new via Serde aliases |

---

## 5. Recommended Implementation Plan

Apply `#[serde(rename_all = "lowercase")]` with backwards-compatible aliases to `src/types.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionScheme {
    #[serde(alias = "Cenc", alias = "CENC")]
    Cenc,
    #[serde(alias = "Cbcs", alias = "CBCS")]
    Cbcs,
    #[serde(alias = "Dual", alias = "DUAL")]
    Dual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackType {
    #[serde(alias = "Video", alias = "VIDEO")]
    Video,
    #[serde(alias = "Audio", alias = "AUDIO")]
    Audio,
    #[serde(alias = "Subtitle", alias = "SUBTITLE")]
    Subtitle,
}
```
