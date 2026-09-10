## 1. Overview

DRM protects media by **encrypting video/audio with a Content Key**, and subsequently providing that Content Key only to authorized playback devices through a **DRM License**.

The entire architecture is divided into two distinct phases:

```text
                    DRM END-TO-END FLOW

        PACKAGING                         PLAYBACK
        ---------                         --------

       Clear Media
            |
            v
        Packager
            |
            | CPIX / SPEKE
            v
       Key Service
            |
            | Content Key
            v
        Encryptor
            |
            v
     Encrypted CMAF
            |
            v
           CDN
            |
            +--------------------------> Player
                                           |
                                           v
                                          CDM
                                           |
                                           | License Request
                                           v
                                     License Service
                                           |
                                           | DRM License
                                           | + same Content Key
                                           v
                                          CDM
                                           |
                                           v
                                        Decrypt
                                           |
                                           v
                                         Decode
                                           |
                                           v
                                        Playback

```

The most fundamental architectural invariant:

> **The Content Key used to decrypt on the device must be the exact same Content Key used to encrypt media during packaging.**

---

# 2. Core Concepts

## 2.1 Content Key

The **Content Key** is the actual secret cryptographic key used to **encrypt and decrypt media**.

In Common Encryption, the Content Key is typically an **AES-128 key**:

- AES = Advanced Encryption Standard.

- AES is a **symmetric encryption** cipher.

- The exact same key is used for both encryption and decryption.

- AES-128 uses a key length of **128 bits = 16 bytes**.

- The Content Key must always be kept strictly confidential.


```text
                    Content Key
                    (AES-128)
                        |
          +-------------+-------------+
          |                           |
          v                           v
       ENCRYPT                     DECRYPT

Clear Media ---------> Ciphertext ---------> Clear Media
              KEY                     SAME KEY

```

Example:

```text
Content Key:
    9f7a...<secret>...81c2

```

---

## 2.2 KID — Key ID

Each Content Key is identified by a unique public identifier called a **KID (Key ID)**.

```text
KID ------- identifies -------> Content Key
                                 (AES-128)

```

Example:

```text
KID:
    67eac2fc-0f60-4cfe-96b2-1f3e572c6457

Content Key:
    9f7a...<secret>...81c2

```

Distinction:

```text
KID          = public identifier
Content Key  = secret key material

```

The KID appears inside media containers and manifest signaling:

```text
Encrypted Media
      |
      | KID = X
      v
"This content requires Content Key X"

```

The KID **does not participate directly in AES encryption arithmetic**. It serves exclusively to identify which key must be retrieved and applied.

---

## 2.3 AES

**AES** is a symmetric block cipher.

```text
             SAME SECRET KEY
              |         |
              v         v

Plaintext -- AES --> Ciphertext -- AES --> Plaintext

```

DRM and Common Encryption standards primarily utilize AES-128.

AES defines the cipher algorithm itself. How AES is applied across media streams depends on the chosen **encryption scheme**.

---

## 2.4 Encryption Scheme — `cenc` and `cbcs`

Two primary Common Encryption schemes are used in production:

```text
cenc
    +-- AES-CTR mode

cbcs
    +-- AES-CBC pattern encryption mode

```

Architectural relationship:

```text
Content Key
     |
     v
    AES
     |
     v
Encryption Scheme
     |
     +-- cenc
     |
     +-- cbcs
     |
     v
Encrypted Media Samples

```

`cbcs` is essential in modern CMAF Multi-DRM workflows to support Apple FairPlay concurrently with Widevine and PlayReady on unified CMAF fragments.

---

## 2.5 IV — Initialization Vector

AES encryption requires more than just the Content Key.

It also requires an **IV (Initialization Vector)** or counter-related state depending on the cipher mode:

```text
Plain Media
    +
Content Key
    +
IV
    |
    v
Encryption
    |
    v
Ciphertext

```

Unlike the Content Key, the IV does not need to be kept secret.

Metadata required to reconstruct the correct IV per sample is embedded directly within the encrypted ISO-BMFF container.

---

## 2.6 DRM System

Major DRM systems across the industry:

```text
Widevine     > Google (Android, Chrome, Smart TVs)

PlayReady    > Microsoft (Windows, Edge, Xbox)

FairPlay     > Apple (iOS, macOS, Safari, Apple TV)

```

A DRM system governs:

- License exchange protocols;

- License container formats;

- Cryptographic key protection;

- Device hardware root of trust;

- Usage rules and playback rights;

- Output protection (e.g. HDCP);

- Secure hardware decoding pipelines.


---

## 2.7 DRM Provider

A **DRM Service Provider** manages cloud DRM infrastructure on behalf of content owners.

Example:

```text
Axinom

```

Conceptually:

```text
DRM Service Provider
|
+-- Key Service
|    +-- serves PACKAGING phase
|
+-- License Service
     +-- serves PLAYBACK phase

```

These two services address distinct stages of the media lifecycle.

---

## 2.8 Key Service

The **Key Service** provisions Content Keys to the Packager / Encryption Engine.

```text
Packager
    |
    | request protection data
    v
Key Service
    |
    | KID
    | Content Key
    | DRM signaling
    v
Packager

```

For instance, Axinom designates this API as the **Key Acquisition API**.

---

## 2.9 CPIX

**CPIX — Content Protection Information Exchange Format** is a standardized XML document specification used to exchange:

```text
Content Keys
KIDs
Encryption Scheme
DRM Signaling
Usage Rules
Key Periods
...

```

CPIX operates strictly on the **content preparation / packaging side**:

```text
Key Service
     |
     | CPIX
     v
Packager

```

Client video players never consume or process CPIX documents.

---

## 2.10 SPEKE

**SPEKE — Secure Packager and Encoder Key Exchange** is an API protocol specification (standardized by AWS) governing authentication and payload delivery between Packagers and Key Services.

Relationship hierarchy:

```text
HTTPS
    v
transport layer

SPEKE
    v
protocol / API contract

CPIX
    v
payload data format

Content Key + DRM information

```

CPIX and SPEKE are complementary: SPEKE defines the REST API protocol, while CPIX defines the XML data payload format.

---

## 2.11 DRM Signaling

A Content Key alone is insufficient for a media player to initiate DRM playback.

Media containers and manifests must convey **DRM signaling**:

```text
                Encrypted Content
                       |
          +------------+------------+
          v            v            v
      Widevine     PlayReady     FairPlay
          |            |            |
        PSSH          PSSH       HLS signaling

```

Signaling informs the player and CDM which DRM systems are supported, which KIDs are required, and supplies the initialization data necessary to initiate license acquisition.

---

## 2.12 PSSH

**PSSH — Protection System Specific Header** is DRM-specific signaling box (`pssh`) embedded in ISO-BMFF / Common Encryption containers.

Conceptually:

```text
PSSH
|
+-- DRM System ID (UUID)
+-- DRM-specific opaque payload

```

Example:

```text
PSSH
|
+-- Widevine System ID (edef8ba9-79d6-4ace-a3c8-27dcd51d21ed)
+-- Widevine initialization payload

```

Important distinctions:

```text
PSSH ≠ Content Key
PSSH ≠ DRM License

```

PSSH is solely initialization and routing signaling.

---

## 2.13 DRM License

A **DRM License** is a DRM-specific, cryptographically wrapped data object issued by the License Service directly to the client's CDM.

Conceptually it encapsulates:

```text
DRM License
|
+-- Content Key(s)
|     +-- cryptographically wrapped by DRM device keys
|
+-- KID(s)
|
+-- Policy
      +-- expiration timestamp
      +-- playback rights
      +-- output restrictions (HDCP levels)
      +-- ...

```

A license is never a plain, unencrypted HTTP response like:

```text
KEY = abc123

```

The Content Key is encrypted under device-specific keys and can only be unpacked inside the CDM's secure environment.

---

## 2.14 CDM

**CDM — Content Decryption Module** is the secure client-side DRM software or hardware engine.

Examples:

```text
Chrome / Android  -> Widevine CDM
Windows / Edge    -> PlayReady CDM
Safari / iOS      -> FairPlay CDM

```

The CDM is responsible for:

```text
license processing
        v
key management
        v
policy enforcement
        v
media decryption

```

Applications and video players never hold or inspect raw Content Keys in unmanaged memory.

---

## 2.15 Authentication

**Authentication** answers the question: **who is the user?**

The application backend verifies user identity via user sessions, JWT access tokens, OAuth2, or proprietary authentication services.

```text
User
 |
 | credentials / access token
 v
Application Backend
 |
 v
Authenticated User
```

Successful authentication **does not imply** the user is entitled to watch every piece of content.

---

## 2.16 Authorization / Entitlement

**Authorization** or **Entitlement** answers the question: **is this authenticated user entitled to receive a DRM License for this specific content / KID?**

Business entitlement rules include:

```text
subscription active status
transactional purchase / rental window
geo-location restrictions
license expiration constraints
concurrent playback stream limits
device security level requirements
```

The authorization decision is encapsulated into a **signed entitlement token/message** verified by the License Service, preventing client-side tampering.

---

## 2.17 Entitlement Service

The **Entitlement Service** evaluates business rules and decides whether to authorize a DRM License acquisition.

```text
Player
  |
  | user session + content_id
  v
Entitlement Service
  |
  +-- authenticate user identity
  +-- check subscription / purchase
  +-- verify content rights
  +-- determine DRM policy
  |
  v
Signed Entitlement
```

In Axinom architecture, Entitlement Service and License Service represent distinct responsibilities: the Entitlement Service authorizes the request; the License Service issues DRM Licenses based on that authorization.

---

## 2.18 Signed Entitlement / License Service Message

In Axinom, the **Entitlement Message** is embedded inside a **License Service Message** and signed as a JWT using **HMAC-SHA256** with a shared **Communication Key**.

```text
Trusted Backend
      |
      | Entitlement Message
      v
License Service Message
      |
      | sign with Communication Key
      v
   Signed JWT
      |
      v
    Player
```

The Communication Key must reside exclusively on the trusted backend and DRM provider infrastructure; **it must never be exposed to browser or mobile clients**. The cryptographic signature proves backend authorization and prevents unauthorized token modification.

---

# 3. Packaging / Encryption Flow

## 3.1 Source Media

Input before packaging:

```text
input.mp4

Video > clear compressed samples
Audio > clear compressed samples

```

Source media contains unencrypted elementary streams.

---

## 3.2 Packager Generates or Selects KID

Single-key scenario:

```text
Video:
KID = UUID-A

```

Multi-key scenario:

```text
Video -----> KID-A > KEY-A
Audio -----> KID-B > KEY-B

```

Resolution-tier multi-key scenario:

```text
SD    > KEY-A
HD    > KEY-B
UHD   > KEY-C
Audio > KEY-D

```

---

# 4. Acquire Content Key

The Packager requires:

```text
KID
Content Key
DRM signaling

```

Example packaging specification:

```text
KID:
    UUID-A

Encryption Scheme:
    cbcs

DRMs:
    Widevine
    PlayReady
    FairPlay

```

The Packager dispatches a key acquisition request to the Key Service.

---

# 5. CPIX / SPEKE Flow

Standard production key exchange workflow:

```text
                 Packager
                    |
                    |
                    | CPIX Request
                    | via SPEKE
                    |
                    v
              +-------------+
              | Key Service |
              |   Axinom    |
              +------+------+
                     |
                     | generate / retrieve
                     | Content Key
                     |
                     v
               CPIX Response
                     |
                     |
                     +-- KID
                     +-- Content Key
                     +-- encryption scheme
                     +-- DRM signaling
                     |
                     v
                  Packager

```

Under SPEKE, the Packager transmits a CPIX document specifying the protection metadata it requires.

The Key Service populates the cryptographic key values and returns the completed CPIX document.

---

# 6. CPIX Request

Conceptual CPIX document:

```xml
<CPIX>

    <ContentKeyList>

        <ContentKey
            kid="UUID-A"
            commonEncryptionScheme="cbcs"/>

    </ContentKeyList>

    <DRMSystemList>

        <DRMSystem
            kid="UUID-A"
            systemId="Widevine"/>

        <DRMSystem
            kid="UUID-A"
            systemId="PlayReady"/>

        <DRMSystem
            kid="UUID-A"
            systemId="FairPlay"/>

    </DRMSystemList>

</CPIX>

```

This request communicates:

```text
"I require a Content Key for KID UUID-A.

The key must be configured for cbcs encryption.

I require signaling metadata for:
    Widevine
    PlayReady
    FairPlay."

```

---

# 7. Key Service

The Key Service ingests the requested KID and generates or retrieves the associated Content Key:

```text
KID = UUID-A
      |
      v
+-------------+
| Key Service |
+------+------+
       |
       v
KEY = SECRET-A

```

The provider may:

```text
generate a cryptographically secure random Content Key

or

derive the Content Key deterministically from:
Key Seed + KID

```

This is an internal implementation detail of the DRM provider.

The critical architectural invariant is: **The License Service must be able to retrieve the exact same Content Key during playback**.

---

# 8. CPIX Response

Conceptual CPIX response structure:

```text
CPIX
|
+-- ContentKey
|     +-- KID = UUID-A
|     +-- KEY = SECRET-A
|     +-- scheme = cbcs
|
+-- DRMSystemList
      |
      +-- Widevine signaling (PSSH)
      +-- PlayReady signaling (PSSH / PRO)
      +-- FairPlay signaling (skd:// URI)

```

The Packager normalizes this data into its internal protection model:

```text
ProtectionData
|
+-- KID
+-- Content Key
+-- Encryption Scheme
+-- DRM Signaling

```

---

# 9. Media Encryption

This is where media sample payload encryption takes place.

The Packager / Encryption Engine consumes:

```text
Media Sample
     +
Content Key
     +
IV
     +
Encryption Scheme

```

and produces:

```text
Encrypted Media Sample

```

Workflow:

```text
Clear H.264 / HEVC sample
          |
          | AES
          | KEY = SECRET-A
          | IV  = ...
          | scheme = cbcs
          v
Encrypted Sample

```

DRM packaging does not encrypt an entire `.mp4` file as an opaque container blob.

Encryption is performed strictly at the **media sample level** within ISO-BMFF fragments.

---

# 10. Common Encryption

**Common Encryption (CENC)** standardizes sample-level encryption formatting so identical encrypted media payloads can be consumed across disparate DRM systems.

```text
                   Content Key
                        |
                        v
                 Common Encryption
                        |
                        v
                  Encrypted CMAF
                        |
            +-----------+-----------+
            v           v           v
        Widevine    PlayReady    FairPlay

```

This eliminates the need to encode, package, and store separate media copies for each DRM vendor.

---

# 11. Sample Encryption

Media samples may contain both unencrypted (clear) and encrypted portions.

```text
Original Sample

+---------------------------------------------+
| Header |       Compressed Payload           |
+---------------------------------------------+


Encrypted Sample

+---------------------------------------------+
| clear  | encrypted | clear | encrypted ...  |
+---------------------------------------------+

```

The exact subsample layout depends on the video codec (e.g. NAL unit headers remain in the clear) and the encryption scheme.

---

# 12. Encryption Metadata in CMAF / ISO-BMFF

The decryption engine requires metadata to answer:

```text
Which Key ID is required?

Which encryption scheme is applied?

Which IV applies to this sample?

Which bytes of the sample are encrypted vs. clear?

```

Encrypted ISO-BMFF / CMAF containers carry this metadata inside standard boxes:

```text
tenc
senc
saiz
saio
pssh

```

---

## 12.1 `tenc`

`tenc` — Track Encryption Box.

```text
Video Track
    |
    +-- tenc
         |
         +-- default encryption parameters
         +-- default_KID = UUID-A

```

It defines default protection parameters for an entire track.

---

## 12.2 `senc`

`senc` — Sample Encryption Box.

It contains per-sample encryption descriptors, including per-sample IVs and subsample byte ranges:

```text
Sample 1
    +-- IV = ...
    +-- subsample byte ranges

Sample 2
    +-- IV = ...
    +-- subsample byte ranges

Sample 3
    +-- IV = ...

```

---

## 12.3 `saiz` / `saio`

`saiz` (Sample Auxiliary Information Sizes) and `saio` (Sample Auxiliary Information Offsets) point parsers directly to sample encryption metadata locations within the fragment.

---

## 12.4 `pssh`

`pssh` carries DRM-specific initialization and signaling payloads:

```text
init.mp4
|
+-- encryption metadata
|
+-- Widevine PSSH
+-- PlayReady PSSH

```

---

# 13. Generate CMAF

Following sample encryption:

```text
Encrypted Samples
       |
       v
CMAF Fragmentation
       |
       v

init.mp4

segment_001.m4s
segment_002.m4s
segment_003.m4s
...

```

Media sample payloads are now encrypted; container structural boxes remain parseable by standard demuxers.

---

# 14. Generate Manifest

The Packager generates streaming manifests:

```text
DASH
    +-- live.mpd / manifest.mpd

HLS
    +-- live.m3u8 / master.m3u8

```

Manifests convey DRM signaling so players can negotiate licenses:

```text
Manifest
|
+-- Content Protection Descriptor
+-- DRM System UUID
+-- KID
+-- Initialization data (PSSH / URI)

```

---

# 15. Distribution

Following packaging, artifacts are published to the CDN:

```text
                       CDN
                        |
       +----------------+-----------------+
       |                |                 |
       v                v                 v
    Manifest        init.mp4          *.m4s
       |                |                 |
 DRM signaling     KID / PSSH        ciphertext

```

These assets are distributed publicly to client devices.

Security does not rely on obscuring these files:

```text
Manifest         ✓ (Public)
Encrypted Media  ✓ (Public)
KID              ✓ (Public)
PSSH             ✓ (Public)

Content Key      ✗ (Secret)

```

**The Content Key is the sole secret that must be protected.**

---

# 16. Playback / Decryption Flow

When playback is requested, an essential gate executes: **application authentication & entitlement** prior to the License Service issuing a DRM License.

```text
                     CDN
                      |
                      v
                  Manifest
                      |
                      v
                    Player
                      |
          +-----------+------------+
          |                        |
          v                        v
 Application Backend             CDM
          |                        |
  authenticate user        DRM Signaling / PSSH
  authorize content               |
          |                        v
          v                 License Challenge
 Entitlement Service              |
          |                        |
          | Signed Entitlement     |
          +-----------+------------+
                      |
                      v
               License Service
                      |
             verify entitlement
             verify requested KID
             apply device/policy rules
                      |
                      v
                 DRM License
                      |
                      v
                     CDM
                      |
               Content Key ready
                      |
                      v
               Encrypted Sample
                      |
                      v
                   Decrypt
                      |
                      v
             Clear Compressed Sample
                      |
                      v
                   Decoder
                      |
                      v
                  Playback
```

While the License Server is a network-accessible endpoint, **possessing the URL and KID is insufficient to obtain a license**. The request must supply a valid signed entitlement and pass DRM-specific challenge validation.

---

# 17. Player Detects DRM

The player fetches:

```text
manifest.mpd

or

live.m3u8 / master.m3u8

```

along with the initialization segment (`init.mp4`).

From DRM signaling and initialization metadata, the player determines:

```text
Is content encrypted?

Which DRM systems are supported?

Which KIDs are required?

What initialization data must be dispatched to the CDM?

```

In web browsers, the player intercepts PSSH data within the ISO-BMFF stream.

---

# 18. EME — Encrypted Media Extensions

On the web platform, JavaScript applications communicate with the CDM via the W3C **EME — Encrypted Media Extensions** standard.

Architecture:

```text
Web Player (Shaka, VideoJS, hls.js)
    |
    v
Browser / EME API
    |
    v
CDM
    |
    +-- Widevine / PlayReady / ...

```

Web applications never implement DRM decryption cryptography directly; they orchestrate workflows through EME.

---

# 19. Initialization Data

The CDM requires **Initialization Data** to construct a license request challenge.

In ISO-BMFF / Common Encryption, initialization data typically corresponds to the PSSH payload.

Browser EME event lifecycle:

```text
Encrypted Media
      |
      | contains initialization data
      v
Browser HTMLMediaElement
      |
      | "encrypted" event
      v
Application / Player
      |
      | initData
      v
CDM

```

Sequence:

```text
encrypted event
      |
      v
event.initData
      |
      v
MediaKeySession.generateRequest(initDataType, event.initData)
      |
      v
CDM

```

---

# 20. MediaKeySession

The browser establishes a **MediaKeySession** representing the cryptographic context:

```text
Initialization Data
       |
       v
MediaKeySession
       |
       v
License Challenge Message
       |
       v
License Response
       |
       v
Keys available to CDM

```

A single session can manage multiple Content Keys associated with distinct KIDs (e.g. separate audio and video keys).

---

# 21. CDM Generates License Challenge

The CDM ingests the initialization data and outputs an opaque, cryptographically signed challenge:

```text
PSSH / Init Data
       |
       v
      CDM
       |
       v
License Challenge

```

The challenge payload is specific to the target DRM system:

```text
Widevine  -> Widevine License Request
PlayReady -> PlayReady License Challenge
FairPlay  -> Server Playback Context (SPC)

```

The application intercepts this message via the `message` event and forwards it to the License Service.

---

# 22. Authentication & Entitlement

Before requesting a DRM License, the application must verify user viewing rights. This is the **business authorization** layer, separate from the CDM's device challenge.

```text
User
 |
 | login / access token
 v
Application Backend
 |
 +-- Authentication
 |      "Who is the user?"
 |
 +-- Authorization / Entitlement
        "Is this user authorized to watch this content?"
 |
 v
Entitlement Service
 |
 +-- subscription / purchase / rental validation
 +-- content / KID permission check
 +-- validity window check
 +-- geo / concurrency policies
 +-- DRM usage rules
 |
 v
Signed Entitlement
```

In Axinom, the License Acquisition API does not rely on HTTP `Authorization` headers. Instead, requests must deliver a **License Service Message** containing a **valid Entitlement Message**.

A signed entitlement binds access rights to Content Keys and policies:

```text
Signed Entitlement
|
+-- authorized KID(s)
+-- validity / expiration window
+-- license lifetime
+-- DRM usage policies
+-- optional device / IP restrictions
```

In Axinom, the Entitlement Message is packaged inside a License Service Message and signed as a JWT with **HMAC-SHA256 + Communication Key**:

```text
                 TRUSTED BACKEND

Entitlement Message
       |
       v
License Service Message
       |
       | HMAC-SHA256
       | Communication Key
       v
   Signed JWT
       |
       v
                 UNTRUSTED CLIENT

       Player receives token
```

The `Communication Key` is a shared secret strictly confined to the trusted backend and Axinom License Service. If this secret leaks to clients, unauthorized actors could forge entitlements.

The Player now holds two independent artifacts:

```text
1. DRM License Challenge
   +-- generated by CDM
       +-- requested KID
       +-- device-specific cryptographic proof

2. Signed Entitlement
   +-- generated by trusted backend
       +-- authorized KID(s)
       +-- validity window
       +-- DRM policy rules
```

The License Service issues a license only when both artifacts align:

```text
CDM requests:       KID = A
Entitlement allows: KID = A

        v

     GRANT
```

whereas:

```text
CDM requests:       KID = B
Entitlement allows: KID = A

        v

      DENY
```

Possessing the License Server URL and KID does not constitute authorization.

---

# 23. License Acquisition

The Player application aggregates both artifacts: the challenge from the CDM and the signed entitlement from the backend.

```text
                     Player / App
                          |
              +-----------+-----------+
              |                       |
              v                       v
      License Challenge        Signed Entitlement
         from CDM              from trusted backend
              |                       |
              +-----------+-----------+
                          |
                          | HTTPS
                          v
                 +-----------------+
                 | License Service |
                 |   e.g. Axinom   |
                 +--------+--------+
                          |
                 validate entitlement signature
                 validate requested KID
                 validate DRM/device challenge
                 apply usage policy
                          |
                          v
                    DRM License
                          |
                          v
                     Application
                          |
                          | session.update(...)
                          v
                         CDM
```

Under Axinom Standard Mode, the signed License Service Message is transmitted via the `X-AxDRM-Message` HTTP header, the `AxDrmMessage` query parameter, or PlayReady Custom Data depending on platform capabilities.

**CPIX is never involved in this exchange.** CPIX belongs exclusively to packaging; entitlement tokens and DRM challenges govern playback licensing.

---

# 24. License Service

The License Service verifies both the **authorization artifact** and the **DRM challenge** before synthesizing a license:

```text
License Challenge
      +
Signed Entitlement
       |
       v
Verify entitlement signature
       |
       +-- invalid -------------> DENY
       v
Check expiration / validity
       |
       +-- expired -------------> DENY
       v
Determine requested KID
       |
       v
Is KID authorized by entitlement?
       |
       +-- no ------------------> DENY
       v
Validate DRM / device / security policy
       |
       +-- fail ----------------> DENY
       v
Find / derive Content Key
       |
       v
Apply DRM Usage Policy
       |
       v
Generate DRM License
```

Verification flow:

```text
KID = UUID-A
      |
      v
License Service
      |
      v
Content Key = SECRET-A
```

`SECRET-A` must be identical to the key used during packaging. The License Service wraps the key within the DRM-specific license format; applications never receive plaintext keys.

The returned DRM License is cryptographically bound to the client CDM hardware.

---

# 25. DRM License Protects Content Key

The License Service **never transmits plaintext Content Keys to client applications**.

```text
                 Content Key
                      |
                      v
             DRM-specific protection
                      |
                      v
                 DRM License
                      |
                      v
                     CDM

```

The license payload defines:

```text
Content Key (encrypted under CDM public/device key)
Rights & Permissions
Expiration Window
Output Restrictions (HDCP enforcement)
DRM Policy Rules

```

---

# 26. CDM Receives License

The application delivers the license response into the CDM.

Under EME:

```text
License Response
       |
       v
MediaKeySession.update(license)
       |
       v
      CDM

```

The CDM unpacks the license within its isolated cryptographic context and activates the key:

```text
CDM Key Store / Session

KID UUID-A
     |
     +-- Content Key SECRET-A

```

The application layer remains completely blind to the plaintext key material.

---

# 27. Decrypt Media Sample

The player continues downloading encrypted CMAF media fragments:

```text
segment_001.m4s
segment_002.m4s
...

```

Each encrypted sample includes:

```text
Encrypted Sample
|
+-- KID association
+-- IV
+-- encryption scheme (cenc or cbcs)
+-- subsample mapping
+-- ciphertext payload

```

The CDM / Decryptor matches:

```text
KID = UUID-A
      |
      v
Locate Key in CDM
      |
      v
SECRET-A

```

Decryption arithmetic:

```text
Ciphertext
    +
Content Key SECRET-A
    +
IV
    +
cenc/cbcs parameters
    |
    v
AES Decryption
    |
    v
Clear Compressed Sample

```

This exactly reverses the packaging encryption transformation.

---

# 28. Decoder

Following DRM decryption, the data is not yet raw video frames.

It remains **compressed elementary media**:

```text
Encrypted H.264 Sample
        |
        | DRM decrypt
        v
Clear H.264 Sample
        |
        | H.264 codec decode
        v
Raw Video Frame

```

Similarly for HEVC:

```text
Encrypted HEVC
      |
      v
DRM Decryption
      |
      v
Clear HEVC
      |
      v
HEVC Decoder
      |
      v
Raw Video Frame

```

Critical conceptual distinction:

```text
DECRYPTION ≠ DECODING

```

Decryption:
```text
ciphertext -> clear compressed media (H.264/HEVC/AAC)

```

Decoding:
```text
compressed media -> raw uncompressed frames (YUV/PCM)

```

---

# 29. Secure Playback Path

At elevated DRM security levels (e.g. Widevine L1, PlayReady SL3000), Content Keys and decrypted media samples are never exposed to general application memory.

```text
              Untrusted User Space
--------------------------------------------------

Application
Browser
Network
Encrypted Media


              Security Boundary (Hardware / TEE)
--------------------------------------------------

                     CDM
                      |
                      | protected key
                      v
                   Decrypt
                      |
                      v
                Secure Decoder
                      |
                      v
               Secure Video Path
                      |
                      v
                   Display (HDCP Protected)

```

Protective hardware mechanisms:

```text
TEE (Trusted Execution Environment)
Hardware-backed key provisioning & storage
Secure Hardware Decoders
Protected Video Pipeline
HDCP link encryption

```

This ensures that neither raw Content Keys nor uncompressed 4K video frames can be scraped from host memory by malicious processes.

---

# 30. Software DRM vs Hardware-backed DRM

Conceptual comparison:

```text
Software DRM (e.g. Widevine L3)

Encrypted Media
      |
      v
Software CDM (User space library)
      |
      v
Software Decryption (Host CPU memory)
      |
      v
Decoder (System media framework)

```

Hardware-backed DRM (e.g. Widevine L1, Apple FairPlay):

```text
Encrypted Media
      |
      v
CDM / Trusted Environment (TEE / Secure Enclave)
      |
      v
Hardware-backed Key Handling
      |
      v
Secure Hardware Decryption
      |
      v
Secure Decoder
      |
      v
Protected Video Pipeline & Display

```

---

# 31. Synchronization Invariant Between Key Service & License Service

This represents the foundational invariant of any DRM architecture:

Packaging phase:

```text
KID UUID-A
      |
      v
Key Service
      |
      v
KEY SECRET-A
      |
      v
Encrypt Media

```

Playback phase:

```text
KID UUID-A
      |
      v
License Service
      |
      v
KEY SECRET-A
      |
      v
Decrypt Media

```

If key synchronization fails:

```text
Encryption Key = SECRET-A

Decryption Key = SECRET-B

```

then:

```text
Ciphertext
    +
SECRET-B
    |
    v
INVALID DATA / CORRUPTED FRAMES

```

Playback immediately fails.

---

# 32. Key Rotation

Live streaming systems do not require using a single Content Key indefinitely.

In live streaming, keys can rotate periodically across time windows:

```text
TIME ----------------------------------------->

Period 1          Period 2          Period 3

KID-A             KID-B             KID-C
KEY-A             KEY-B             KEY-C

segment 1-10      segment 11-20     segment 21-30

```

The player and CDM acquire and activate keys dynamically as playback crosses period boundaries.

CPIX natively defines key period specifications for packagers.

---

# 33. Multi-DRM

Multi-DRM does not require duplicating video storage:

```text
Widevine
    > encrypted copy A (NOT REQUIRED)

PlayReady
    > encrypted copy B (NOT REQUIRED)

FairPlay
    > encrypted copy C (NOT REQUIRED)

```

Common Encryption enables:

```text
                  Encrypted CMAF
                        |
           +------------+------------+
           v            v            v
       Widevine     PlayReady     FairPlay

```

provided that target DRMs and client platforms support the shared scheme (e.g. `cbcs`).

Platform variations exist solely in:

```text
DRM signaling metadata
License protocols
License container formats
CDM implementations
Hardware security certifications
Policy enforcement mechanisms

```

The underlying media ciphertext remains 100% identical.

---

# 34. Security Boundaries

Data assets fall into three security classifications:

### Public / Distributable Assets

```text
Encrypted CMAF fragments (.m4s)
Manifest files (.mpd, .m3u8)
KID (Key ID)
PSSH boxes
DRM signaling descriptors

```

### Secret Credentials

```text
Content Keys
Key Seed material
DRM provider API credentials
Entitlement signing secret keys (Communication Keys)

```

### DRM-Protected Material

```text
DRM License tokens
Device credentials / private keys
Internal CDM state
Hardware-wrapped key storage

```

Content Keys must never be:

```text
printed to console or stdout logs
written to unencrypted disk storage without necessity
included in telemetry metrics
embedded in operational error messages
exposed over client-facing application APIs

```

---

# 35. End-to-End Technical Flow

```text
                        CONTENT PREPARATION
==============================================================

                       Clear Media
                            |
                            v
                        Packager
                            |
                    Generate/select KID
                            |
                            | CPIX Request
                            | via SPEKE
                            v
                  +-------------------+
                  |  DRM Key Service  |
                  |   e.g. Axinom     |
                  +---------+---------+
                            |
                            | CPIX Response
                            +-- KID
                            +-- Content Key
                            +-- DRM signaling
                            |
                            v
                   Encryption Engine
                        e.g. GPAC
                            |
              Media Sample + Content Key
                     + IV + cenc/cbcs
                            |
                            v
                    Encrypted Samples
                            |
                            v
                     CMAF Packaging
                            |
               +------------+------------+
               v                         v
        Encrypted CMAF              HLS / DASH
        .mp4 / .m4s                 Manifest
               |                         |
               +------------+------------+
                            v
                           CDN


                 AUTHENTICATION / ENTITLEMENT
==============================================================

                           User
                            |
                     login / session
                            v
                  Application Backend
                            |
                    authenticate user
                            |
                            v
                  Entitlement Service
                            |
                 check business rules:
                 subscription / purchase
                 content permission
                 expiration / geo / device
                            |
                            v
                    Entitlement Message
                            |
                  wrap + sign server-side
                  (Axinom: HMAC-SHA256
                   + Communication Key)
                            |
                            v
                    Signed Entitlement


                           PLAYBACK
==============================================================

                           CDN
                            |
                            v
                          Player
                            |
                     DRM Signaling
                            |
                            v
                    Initialization Data
                       e.g. PSSH
                            |
                            v
                           CDM
                            |
                    Generate Challenge
                            |
                            v
                    License Challenge
                            |
            +---------------+----------------+
            |                                |
            |                        Signed Entitlement
            |                                |
            +---------------+----------------+
                            v
                  +--------------------+
                  | DRM License Service|
                  |    e.g. Axinom     |
                  +---------+----------+
                            |
                  verify entitlement signature
                  validate expiration
                  validate requested KID
                  validate device/security policy
                            |
                            v
                    Find / derive SAME
                       Content Key
                            |
                            v
                       DRM License
                    + DRM policies
                            |
                            v
                           CDM
                            |
                     Content Key ready
                            |
                            v
                   Encrypted CMAF Sample
                            |
                     KID + IV + scheme
                            |
                            v
                      AES Decryption
                            |
                            v
                  Clear Compressed Sample
                            |
                            v
                          Decoder
                            |
                            v
                     Raw Video / Audio
                            |
                            v
                        > Playback
```

---

# 36. Mental Model

If you retain only one complete flow:

```text
1. KEY ACQUISITION

Packager ----- CPIX/SPEKE -----> Key Service
         <----------------------
             Content Key


2. ENCRYPTION

Clear Media
    +
Content Key
    +
IV
    +
cenc/cbcs
    |
    v
Encrypted CMAF


3. DISTRIBUTION

Encrypted CMAF + Manifest
    |
    v
   CDN


4. USER AUTHORIZATION

User -----> Application Backend / Entitlement Service
            |
            | authenticate + business authorization
            v
       Signed Entitlement


5. DRM LICENSE ACQUISITION

CDM -------> License Challenge -------+
                                         |
Signed Entitlement ---------------------+
                                         v
                                  License Service
                                         |
                              validate entitlement
                              + KID/device/policy
                                         |
                                         v
                                    DRM License
                                + SAME Content Key
                                         |
                                         v
                                        CDM


6. DECRYPTION

Encrypted Sample
    +
Content Key
    +
IV
    |
    v
Clear Compressed Sample


7. DECODING

Clear Compressed Sample
    |
    v
Codec Decoder
    |
    v
Video / Audio
    |
    v
Playback
```

Or in the most concise summary:

```text
PACKAGING

Key Service -- Content Key --> Encryptor --> Encrypted Media


PLAYBACK AUTHORIZATION

User --> Backend / Entitlement Service --> Signed Entitlement


PLAYBACK DRM

CDM -- License Challenge --+
                           +--> License Service -- DRM License --> CDM
Entitlement ----------------+                              |
                                                          | same key
                                                          v
                                              Encrypted Media --> Decrypt
```

This represents the architectural backbone of end-to-end DRM systems:

- **Key Service provisions the Content Key to the Encryptor during packaging.**
- **Entitlement Service determines whether the user/client is entitled to request a license for a specific content/KID.**
- **License Service issues a DRM License only after valid authorization and compliant DRM policy evaluation.**
- **License Service delivers the exact same Content Key to the client CDM, securely wrapped by DRM.**
- **CPIX/SPEKE handles key exchange on the packaging side.**
- **Signed Entitlement + DRM License Challenge orchestrates authorization and license acquisition on the playback side.**
- **Widevine / PlayReady / FairPlay manages license formatting, key protection, device trust, and policy enforcement on playback devices.**
- **Common Encryption (`cenc` / `cbcs`) specifies how media samples are encrypted and decrypted.**
- **The CDM retains and utilizes keys to decrypt media inside the secure client environment.**

---

# 37. References

- DASH-IF CPIX Specification: https://dashif.org/CPIX/
- W3C Encrypted Media Extensions (EME): https://www.w3.org/TR/encrypted-media/
- Axinom DRM License Service: https://docs.axinom.com/services/drm/license-service/
- Axinom License Acquisition API: https://docs.axinom.com/services/drm/license-service/license-acquisition-api
- Axinom — Sign License Service Message: https://docs.axinom.com/services/drm/how-to-guides/sign-license-service-message
- Axinom — What is DRM?: https://docs.axinom.com/services/drm/general/what-is-drm/
