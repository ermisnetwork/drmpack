# Research Report: Industry Standards & Best Practices for Audio DRM Key Naming, Quality Tiers, and Track Types in Streaming Systems

- **Document:** `docs/research/audio-drm-quality-tier-standards.md`
- **Date:** 2026-09-10
- **Status:** Completed & Validated Against Primary Specifications (DASH-IF CPIX 2.3, AWS SPEKE v2, Axinom DRM, Unified Streaming, Shaka Packager)

---

## 1. Executive Summary & Core Findings

### 1.1 The Problem
In `drmpack`:
1. `Rendition::audio()` in `src/types.rs` defaults to `QualityTier::sd()`.
2. `src/cpix/builder.rs` constructs audio usage rules as `intendedTrackType="AUDIO_{quality_tier}"`, which outputs `intendedTrackType="AUDIO_SD"`.
3. `src/cpix/parser.rs` hardcodes `"SD" | "AUDIO" => QualityTier::sd()`, forcing any inbound `"AUDIO"` key to be converted to `QualityTier("SD")`.
4. When developers inspect `DrmStreamMetadata` emitted by `session.playback_metadata()`, audio keys display `track_type: "audio"` with `quality_tier: "SD"`. This causes immediate confusion: *"Why does an audio track have an SD (Standard Definition) video resolution tier?"*

### 1.2 Core Takeaways from Primary Standards
1. **DASH-IF CPIX 2.3**: Differentiates audio from video strictly using filter child elements: `<cpix:AudioFilter>` vs `<cpix:VideoFilter>`. `intendedTrackType` is optional business metadata.
2. **AWS SPEKE v2**: Elevates `intendedTrackType` to a mandatory, strictly validated attribute:
   - Single / all audio tracks: **`intendedTrackType="AUDIO"`** with `<cpix:AudioFilter />`.
   - Multi-channel audio tracks: **`"STEREO_AUDIO"`**, **`"MULTICHANNEL_AUDIO"`**, **`"MULTICHANNEL_AUDIO_3_6"`**, and **`"MULTICHANNEL_AUDIO_7"`**.
   - Video tracks: `"VIDEO"`, `"SD"`, `"HD"`, `"UHD"`, `"UHD1"`, `"UHD2"`, etc.
   - **`"AUDIO_SD"` does not exist in any AWS specification or preset.**
3. **Axinom Key Service & Axinom DRM**:
   - Axinom SPEKE 2.0 implementation and samples explicitly use **`intendedTrackType="AUDIO"`** with `<cpix:AudioFilter />` and `"VIDEO"` with `<cpix:VideoFilter />`. Usage rules are a pass-through.
   - Axinom DRM License Service separates packaging keys from license entitlement policies: each key references a `usage_policy` (e.g. `"audio"` or default). Audio policies enforce zero HDCP (`hdcp: "NONE"`) and software CDM (`SW_SECURE_CRYPTO` / L3), whereas video policies enforce HDCP (Type 0 / Type 1) and hardware CDMs.
4. **Historical SD Mapping Rationale**: MovieLabs Enhanced Content Protection (ECP) rules require HDCP 2.2 and hardware DRM for UHD/4K, HDCP 1.4 for HD, but allow software DRM (Widevine L3, PlayReady SL2000) and no HDCP for SD. Because audio must play synchronously with any video resolution on any device (including L3 software devices), audio must be licensed under baseline security rules. Early systems (SPEKE v1) bundled audio with the SD video key or labeled the audio key's security policy as "SD".
5. **Architectural Recommendation for `drmpack`**:
   - Introduce dedicated **`QualityTier::audio()`** with value `"AUDIO"`.
   - Default `Rendition::audio()` to `QualityTier::audio()`.
   - Update `builder.rs` so that audio emits `intendedTrackType="AUDIO"` (or channel descriptors), matching SPEKE v2, Axinom, and Shaka Packager.
   - Update `parser.rs` to map `"AUDIO" => QualityTier::audio()`.
   - In `DrmStreamMetadata`, audio keys serialize cleanly as `quality_tier: "AUDIO"`.

---

## 2. Primary Source Investigations

### 2.1 DASH-IF CPIX 2.3 Specification
- **Primary Source:** [DASH-IF CPIX 2.3 (Section 5.2.12, 5.2.13.3, 5.2.13.4, 5.2.13.5)](https://dashif.org/docs/CPIX2.3/Cpix.html)

#### Key Mechanism & Schema:
In CPIX 2.3, `<cpix:ContentKeyUsageRule>` binds a Content Key (`kid`) to media tracks using child filter elements:
- `<cpix:VideoFilter>`: Matches video samples. Attributes include `minPixels`, `maxPixels`, `hdr`, `wcg`, `minFps`, `maxFps`. If present without attributes, matches all video samples.
- `<cpix:AudioFilter>`: Matches audio samples. Attributes include `minChannels`, `maxChannels`. If present without attributes, matches all audio samples.
- `intendedTrackType` (`xs:string`, Optional): Section 5.2.12 & 5.2.13.3 define this as high-level business metadata:
  > *"In contrast, the `intendedTrackType` attribute of ContentKeyUsageRule is used to assign a track type to the media streams which match the filters. The value of the string may not be pre-agreed... Said differently, the `intendedTrackType` attribute is a metadata that states business logic... It has no function in defining what Content Keys are matched to what tracks, it simply acts as a label to allow business logic to say authorize the use of `lowRes` Content Key and then a CPIX processor can find the rules that match the right Content Keys."*

---

### 2.2 AWS SPEKE v2 & AWS Elemental MediaConvert / MediaPackage
- **Primary Sources:**
  - [AWS SPEKE v2 Encryption Contract](https://docs.aws.amazon.com/speke/latest/documentation/encryption-contract-v2.html)
  - [AWS Elemental MediaConvert SPEKE v2 Presets](https://docs.aws.amazon.com/mediaconvert/latest/ug/drm-content-speke-v2-presets.html)
  - [AWS Elemental MediaPackage SPEKE v2 Presets](https://docs.aws.amazon.com/mediapackage/latest/ug/speke-v2-presets.html)

#### SPEKE v2 Encryption Contract Principles:
1. `ContentKeyUsageRule@intendedTrackType` is **mandatory** and must be unique in an encryption contract.
2. Sub-components are joined by `+` (e.g. `"SD+HD"`).
3. **Cardinality rule:** The number of `<cpix:AudioFilter>` and `<cpix:VideoFilter>` elements must correspond exactly to the number of sub-components in `intendedTrackType`.
4. When all audio and video share a single key, **`"ALL"`** must be used, accompanied by both an empty `<cpix:AudioFilter />` and `<cpix:VideoFilter />`.

#### Standard Values for Audio:
From the SPEKE v2 specification examples and MediaConvert/MediaPackage presets:
- **Preset `PRESET_AUDIO_1` (1 key for all audio)**:
  ```xml
  <cpix:ContentKeyUsageRule kid="53abdba2-f210-43cb-bc90-f18f9a890a02" intendedTrackType="AUDIO">
      <cpix:AudioFilter />
  </cpix:ContentKeyUsageRule>
  ```
- **Preset `PRESET_AUDIO_2` (2 keys: stereo vs surround)**:
  - `intendedTrackType="STEREO_AUDIO"` with `<cpix:AudioFilter maxChannels="2" />`
  - `intendedTrackType="MULTICHANNEL_AUDIO"` with `<cpix:AudioFilter minChannels="3" />`
- **Preset `PRESET_AUDIO_3` (3 keys: stereo, 5.1 surround, 7.1/spatial)**:
  - `intendedTrackType="STEREO_AUDIO"` with `<cpix:AudioFilter maxChannels="2" />`
  - `intendedTrackType="MULTICHANNEL_AUDIO_3_6"` with `<cpix:AudioFilter minChannels="3" maxChannels="6" />`
  - `intendedTrackType="MULTICHANNEL_AUDIO_7"` with `<cpix:AudioFilter minChannels="7" />`

#### Standard Values for Video:
- `VIDEO` (1 key for all video)
- `SD` (`maxPixels="589824"`, i.e. <= 1024x576)
- `HD` / `HD1` / `HD2` (720p / 1080p)
- `UHD` / `UHD1` / `UHD2` (4K / 8K)

**Conclusion:** In AWS SPEKE v2, audio is **never** given a video resolution name like `"SD"` or `"AUDIO_SD"`. The universal standard name is **`"AUDIO"`**.

---

### 2.3 Historical Rationale: Why Audio was Mapped to or Bundled with "SD"

1. **MovieLabs Enhanced Content Protection (ECP) & Studio Robustness Rules:**
   - Hollywood studios mandate strict hardware isolation and output protection for high-value video:
     - **UHD / 4K / HDR**: Requires Hardware Root of Trust, Secure Media Path, Hardware Video Decoder, and **HDCP 2.2+ (Type 1)** on all digital display outputs.
     - **HD (720p / 1080p)**: Requires standard DRM and **HDCP 1.4+ (Type 0)**.
     - **SD (<= 576p)**: Permitted on Software DRM (Widevine L3 in Chrome/Firefox/Electron, PlayReady SL2000), with **no HDCP** required.
2. **Audio's Operational Invariance:**
   - Audio must play concurrently across **all** video renditions. A user streaming an SD video rendition on a PC with an older non-HDCP VGA/DVI monitor or software CDM must be able to decrypt the audio track.
   - If audio were encrypted with an HD or UHD key, any software CDM (Widevine L3) or non-HDCP client would be refused the audio key by the DRM license server, resulting in muted playback.
   - Therefore, audio encryption keys must be decryptable by the lowest-capability device that can play any video: the "SD" security policy tier.
3. **Legacy Packaging Architecture (SPEKE v1 & Early Multi-Key):**
   - SPEKE v1 supported only a single key for all media.
   - Early multi-key systems used 2 keys (`Key 1 = SD Video + Audio`, `Key 2 = HD Video`) or 3 keys (`Key 1 = SD Video + Audio`, `Key 2 = HD Video`, `Key 3 = UHD Video`).
   - Even when packaging tools later decoupled audio onto a separate physical Key ID, DRM license servers mapped the audio key ID to the existing "SD" license policy (zero HDCP, software L3 allowed). Many workflows continued referring to the audio key as the "SD key" or "SD tier".

---

### 2.4 Axinom Key Service & Axinom DRM

- **Primary Sources:**
  - [Axinom Key Service SPEKE Documentation & SPEKE 2.0 Samples](https://docs.axinom.com/services/drm/key-service/speke/)
  - [Axinom DRM License Service Entitlement Message & Content Key Usage Policies](https://docs.axinom.com/services/drm/license-service/entitlement-message/content-key-usage-policies)

1. **Axinom Key Service (`/SpekeV2`)**:
   - The official Axinom SPEKE 2.0 sample request and response use:
     ```xml
     <cpix:ContentKeyUsageRuleList>
         <cpix:ContentKeyUsageRule kid="98ee5596-cd3e-a20d-163a-e382420c6eff" intendedTrackType="VIDEO">
             <cpix:VideoFilter />
         </cpix:ContentKeyUsageRule>
         <cpix:ContentKeyUsageRule kid="53abdba2-f210-43cb-bc90-f18f9a890a02" intendedTrackType="AUDIO">
             <cpix:AudioFilter />
         </cpix:ContentKeyUsageRule>
     </cpix:ContentKeyUsageRuleList>
     ```
   - Axinom documentation confirms that `ContentKeyUsageRuleList` is treated as a pass-through: Axinom validates key generation via the Key Seed model and returns the rules unchanged to the packager.
2. **Axinom DRM License Service & Entitlement Messages**:
   - Axinom cleanly decouples key generation from playback entitlement. In an Axinom Entitlement Message (JWT):
     - Each key in `content_keys_source.inline` references a `usage_policy` by name (e.g. `"usage_policy": "audio"` or `"usage_policy": "sd"`).
     - `content_key_usage_policies` sets DRM-specific parameters:
       - **FairPlay**: `hdcp` (`"NONE"`, `"TYPE0"`, `"TYPE1"`, `"TYPE1_STRICT"`), `min_security_level` (`"Baseline"`, `"Main"`).
       - **Widevine**: `device_security_level` (`"SW_SECURE_CRYPTO"`, `"HW_SECURE_ALL"`), `hdcp` (`"NONE"`, `"2.2"`).
       - **PlayReady**: `compressed_digital_audio_opl`, `uncompressed_digital_audio_opl`, `digital_audio_output_protections` (SCMS copy bits).
   - In Axinom best practice, audio is assigned an independent policy (`"audio"`), with software crypto and zero HDCP. It is not coupled to SD.

---

### 2.5 Industry Packagers (Unified Streaming & Shaka Packager)

1. **Unified Streaming (Unified Packager & Unified Origin)**:
   - Evaluates CPIX documents using `<VideoFilter>` and `<AudioFilter>`.
   - Recommends encrypting audio with a separate key from video so that license policies do not inadvertently block audio on low-capability devices.
2. **Google Shaka Packager**:
   - Standardizes DRM labels: `--keys label=AUDIO:key_id=...:key=...` alongside `SD`, `HD`, `UHD1`, `UHD2`.
   - Automatically maps `stream=audio` to DRM label `AUDIO`.

---

## 3. Recommendations for `drmpack`

| Dimension | Current Codebase | Recommended Solution |
| :--- | :--- | :--- |
| **`QualityTier` Constructor** | Only `sd()`, `hd()`, `uhd_4k()` | Add `QualityTier::audio()` -> `"AUDIO"` |
| **`Rendition::audio()`** | Defaults to `QualityTier::sd()` | Defaults to `QualityTier::audio()` |
| **CPIX Request** | Emits `intendedTrackType="AUDIO_SD"` | Emits `intendedTrackType="AUDIO"` |
| **CPIX Response Parser** | `"SD" \| "AUDIO" => QualityTier::sd()` | Maps `"AUDIO" => QualityTier::audio()` |
| **Metadata Output** | `"quality_tier": "SD"` for audio | `"quality_tier": "AUDIO"` for audio |
