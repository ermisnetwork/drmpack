## 1. Tổng quan

DRM bảo vệ media bằng cách **mã hóa video/audio bằng một Content Key**, sau đó chỉ cung cấp Content Key đó cho thiết bị được phép phát thông qua một **DRM License**.

Toàn bộ hệ thống chia thành hai giai đoạn:

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

Điểm quan trọng nhất:

> **Content Key dùng để decrypt trên thiết bị phải chính là Content Key đã dùng để encrypt media lúc packaging.**

---

# 2. Core Concepts

## 2.1 Content Key

**Content Key** là khóa bí mật thực sự dùng để **mã hóa và giải mã media**.

Trong Common Encryption, Content Key thường là **AES-128 key**:

- AES = Advanced Encryption Standard.

- AES là **symmetric encryption** — mã hóa đối xứng.

- Cùng một key được dùng để encrypt và decrypt.

- AES-128 sử dụng key dài **128 bit = 16 bytes**.

- Content Key phải được giữ bí mật.


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

Ví dụ:

```text
Content Key:
    9f7a...<secret>...81c2

```

---

## 2.2 KID — Key ID

Mỗi Content Key có một identifier gọi là **KID (Key ID)**.

```text
KID ------- identifies -------> Content Key
                                 (AES-128)

```

Ví dụ:

```text
KID:
    67eac2fc-0f60-4cfe-96b2-1f3e572c6457

Content Key:
    9f7a...<secret>...81c2

```

Khác biệt:

```text
KID          = public identifier
Content Key  = secret

```

KID có thể xuất hiện trong media hoặc signaling.

```text
Encrypted Media
      |
      | KID = X
      v
"Content này cần Content Key X"

```

KID **không tham gia trực tiếp vào phép AES encryption**. Nó dùng để xác định key cần sử dụng.

---

## 2.3 AES

**AES** là thuật toán mã hóa đối xứng.

```text
             SAME SECRET KEY
              |         |
              v         v

Plaintext -- AES --> Ciphertext -- AES --> Plaintext

```

DRM/Common Encryption thường sử dụng AES-128.

AES chỉ định cipher. Cách AES được áp dụng lên media phụ thuộc **encryption scheme**.

---

## 2.4 Encryption Scheme — `cenc` và `cbcs`

Hai scheme thường gặp:

```text
cenc
    +-- AES-CTR based

cbcs
    +-- AES-CBC pattern encryption

```

Có thể hiểu:

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

`cbcs` đặc biệt quan trọng trong các workflow CMAF Multi-DRM cần hỗ trợ FairPlay cùng Widevine/PlayReady.

---

## 2.5 IV — Initialization Vector

AES encryption không chỉ sử dụng Content Key.

Nó còn cần **IV (Initialization Vector)** hoặc counter-related state tùy encryption mode.

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

IV không cần được giữ bí mật như Content Key.

Thông tin cần thiết để sử dụng đúng IV được lưu trong encrypted media metadata.

---

## 2.6 DRM System

Các DRM system phổ biến:

```text
Widevine     > Google

PlayReady    > Microsoft

FairPlay     > Apple

```

DRM system chịu trách nhiệm cho những thứ như:

- license protocol;

- license format;

- key protection;

- device security;

- usage policy;

- output protection;

- secure playback.


---

## 2.7 DRM Provider

Một **DRM Service Provider** cung cấp infrastructure DRM.

Ví dụ:

```text
Axinom

```

Conceptually:

```text
DRM Service Provider
|
+-- Key Service
|
|    +-- phục vụ PACKAGING
|
+-- License Service
     +-- phục vụ PLAYBACK

```

Hai service này có vai trò khác nhau.

---

## 2.8 Key Service

**Key Service** cung cấp Content Key cho Packager/Encryption Engine.

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

Ví dụ Axinom gọi API này là **Key Acquisition API**.

---

## 2.9 CPIX

**CPIX — Content Protection Information Exchange Format** là một XML format chuẩn hóa để trao đổi:

```text
Content Keys
KIDs
Encryption Scheme
DRM Signaling
Usage Rules
Key Periods
...

```

CPIX thuộc **content preparation / packaging side**.

```text
Key Service
     |
     | CPIX
     v
Packager

```

Player không sử dụng CPIX để playback.

---

## 2.10 SPEKE

**SPEKE — Secure Packager and Encoder Key Exchange** là protocol/API contract dùng để trao đổi protection information giữa Packager và Key Service.

Có thể nhớ:

```text
HTTPS
    v
transport

SPEKE
    v
protocol / API contract

CPIX
    v
data format

Content Key + DRM information

```

CPIX và SPEKE không phải cùng một thứ.

---

## 2.11 DRM Signaling

Content Key chưa đủ để player biết cách sử dụng DRM.

Media/manifest còn cần **DRM signaling**.

Ví dụ:

```text
                Encrypted Content
                       |
          +------------+------------+
          v            v            v
      Widevine     PlayReady     FairPlay
          |            |            |
        PSSH          PSSH       HLS signaling

```

Signaling giúp player/CDM biết DRM system và information cần thiết để bắt đầu license acquisition.

---

## 2.12 PSSH

**PSSH — Protection System Specific Header** là DRM-specific signaling được sử dụng trong ISO-BMFF/Common Encryption workflows.

Conceptually:

```text
PSSH
|
+-- DRM System ID
+-- DRM-specific data

```

Ví dụ:

```text
PSSH
|
+-- Widevine System ID
+-- Widevine initialization data

```

PSSH:

```text
≠ Content Key
≠ DRM License

```

Nó là signaling / initialization information.

---

## 2.13 DRM License

**DRM License** là object DRM-specific được License Service cấp cho CDM.

Conceptually nó có thể chứa:

```text
DRM License
|
+-- Content Key(s)
|     +-- được bảo vệ bởi DRM
|
+-- KID(s)
|
+-- Policy
      +-- expiration
      +-- playback rights
      +-- output restrictions
      +-- ...

```

License không đơn giản là HTTP response:

```text
KEY = abc123

```

Content Key được DRM bảo vệ và được xử lý bởi CDM.

---

## 2.14 CDM

**CDM — Content Decryption Module** là component DRM phía thiết bị.

Ví dụ:

```text
Chrome / Android
       |
       +-- Widevine CDM

```

CDM chịu trách nhiệm cho:

```text
license processing
        v
key management
        v
policy enforcement
        v
media decryption

```

Application/player không nên trực tiếp nhận plaintext Content Key.

---

## 2.15 Authentication

**Authentication** trả lời câu hỏi: **user là ai?**

Ví dụ application backend xác thực user bằng session, access token, OAuth hoặc cơ chế đăng nhập riêng của hệ thống.

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

Authentication thành công **không có nghĩa** user được quyền xem mọi content.

---

## 2.16 Authorization / Entitlement

**Authorization** hoặc **Entitlement** trả lời câu hỏi: **user này có quyền nhận DRM License cho content/KID này hay không?**

Business rules có thể gồm:

```text
subscription
purchase / rental
geo restriction
license expiration
concurrent stream limit
device / security-level policy
```

Kết quả authorization thường được biểu diễn bằng một **signed entitlement token/message** để License Service có thể kiểm tra mà client không thể tự sửa quyền truy cập.

---

## 2.17 Entitlement Service

**Entitlement Service** là component áp dụng business rules và quyết định có cấp quyền xin DRM License hay không.

```text
Player
  |
  | user/session + content_id
  v
Entitlement Service
  |
  +-- authenticate / identify user
  +-- check subscription / purchase
  +-- check content permission
  +-- decide DRM policy
  |
  v
Signed Entitlement
```

Với Axinom, Entitlement Service và License Service là hai trách nhiệm tách biệt: Entitlement Service authorize request; License Service phát DRM License dựa trên authorization đó.

---

## 2.18 Signed Entitlement / License Service Message

Với Axinom, **Entitlement Message** được đặt trong một **License Service Message** rồi ký thành JWT bằng **HMAC-SHA256** với **Communication Key**.

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

Communication Key phải chỉ tồn tại ở trusted backend/provider side, **không được ship xuống browser hoặc mobile client**. Chữ ký chứng minh entitlement do backend được tin cậy phát hành và giúp License Service phát hiện token bị sửa.

---

# 3. Packaging / Encryption Flow

## 3.1 Source Media

Ban đầu:

```text
input.mp4

Video > clear compressed samples
Audio > clear compressed samples

```

Media chưa được DRM encrypt.

---

## 3.2 Packager tạo hoặc chọn KID

Ví dụ:

```text
Video:

KID = UUID-A

```

Hoặc multi-key:

```text
Video -----> KID-A > KEY-A

Audio -----> KID-B > KEY-B

```

Có thể phức tạp hơn:

```text
SD    > KEY-A
HD    > KEY-B
UHD   > KEY-C
Audio > KEY-D

```

---

# 4. Acquire Content Key

Packager cần:

```text
KID
Content Key
DRM signaling

```

Ví dụ yêu cầu:

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

Packager gửi request tới Key Service.

---

# 5. CPIX / SPEKE Flow

Một workflow phổ biến:

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

Trong SPEKE, Packager có thể gửi CPIX document mô tả những protection information nó cần nhưng chưa có giá trị key.

Key Service bổ sung key và trả CPIX lại.

---

# 6. CPIX Request

Conceptually:

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

Có thể hiểu request này là:

```text
"Tôi cần Content Key cho KID UUID-A.

Key sẽ được dùng với cbcs.

Tôi cần signaling cho:
    Widevine
    PlayReady
    FairPlay."

```

---

# 7. Key Service

Key Service nhận KID và tạo hoặc lấy Content Key:

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

Provider có thể:

```text
generate random Content Key

hoặc

derive Content Key từ
Key Seed + KID

```

Đây là implementation detail của DRM provider.

Điều bắt buộc về mặt hệ thống là **License Service sau này phải lấy được chính Content Key đó**.

---

# 8. CPIX Response

Response conceptually:

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
      +-- Widevine signaling
      +-- PlayReady signaling
      +-- FairPlay signaling

```

Packager normalize thông tin này thành internal protection model:

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

Đây mới là nơi actual encryption xảy ra.

Packager/Encryption Engine nhận:

```text
Media Sample
     +
Content Key
     +
IV
     +
Encryption Scheme

```

và tạo:

```text
Encrypted Media Sample

```

Ví dụ:

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

DRM packaging không đơn giản là encrypt toàn bộ `.mp4` như encrypt một ZIP file.

Encryption xảy ra ở **media sample level**.

---

# 10. Common Encryption

**Common Encryption (CENC)** chuẩn hóa cách encrypted media được biểu diễn để cùng encrypted content có thể được sử dụng bởi nhiều DRM systems.

Conceptually:

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

Điều này giúp tránh việc phải tạo một encrypted copy riêng cho mỗi DRM.

---

# 11. Sample Encryption

Media sample có thể gồm phần clear và encrypted.

Conceptually:

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

Exact layout phụ thuộc codec và encryption scheme.

---

# 12. Encryption Metadata trong CMAF / ISO-BMFF

Decryptor cần biết:

```text
Key nào?

Encryption scheme nào?

IV nào?

Phần nào của sample được encrypt?

```

Do đó encrypted ISO-BMFF/CMAF chứa encryption metadata.

Các box thường gặp:

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

Conceptually:

```text
Video Track
    |
    +-- tenc
         |
         +-- encryption defaults
         +-- default_KID = UUID-A

```

Nó giúp xác định protection parameters mặc định của track.

---

## 12.2 `senc`

`senc` — Sample Encryption Box.

Nó có thể chứa per-sample encryption information như IV và subsample information.

```text
Sample 1
    +-- IV = ...

Sample 2
    +-- IV = ...

Sample 3
    +-- IV = ...

```

---

## 12.3 `saiz` / `saio`

Các auxiliary information boxes giúp xác định vị trí/kích thước sample auxiliary encryption data.

Chúng hỗ trợ parser tìm encryption metadata cần thiết cho từng sample.

---

## 12.4 `pssh`

PSSH mang DRM-specific initialization/signaling information.

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

Sau encryption:

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

Media payload đã encrypted.

---

# 14. Generate Manifest

Packager cũng tạo manifest:

```text
DASH
    +-- manifest.mpd

HLS
    +-- master.m3u8

```

Manifest chứa DRM signaling cần thiết để player biết content được bảo vệ như thế nào.

Conceptually:

```text
Manifest
|
+-- Content Protection
+-- DRM System
+-- KID
+-- DRM initialization information

```

---

# 15. Distribution

Sau packaging:

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

Những thứ này có thể được phân phối tới client.

Security không dựa vào việc giấu chúng.

Attacker có thể có:

```text
Manifest         ✓
Encrypted Media  ✓
KID              ✓
PSSH             ✓

Content Key      ✗

```

**Content Key mới là secret quan trọng.**

---

# 16. Playback / Decryption Flow

Khi user nhấn Play, playback có thêm một bước quan trọng: **application authorization / entitlement** trước khi License Service cấp DRM License.

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

License Server có thể là public-facing endpoint, nhưng **biết URL + KID không đủ để nhận license**. Request còn phải thỏa authorization/entitlement và DRM-specific validation.

---

# 17. Player phát hiện DRM

Player tải:

```text
manifest.mpd

hoặc

master.m3u8

```

và init segment.

Từ DRM signaling / initialization data, player xác định:

```text
content encrypted?

DRM system nào?

KID nào?

initialization data nào cần đưa cho DRM?

```

Ví dụ browser có thể gặp PSSH trong ISO-BMFF content.

---

# 18. EME — Encrypted Media Extensions

Trên Web, JavaScript application giao tiếp với CDM thông qua **EME — Encrypted Media Extensions**.

Architecture:

```text
Web Player
    |
    v
Browser / EME
    |
    v
CDM
    |
    +-- Widevine / PlayReady / ...

```

Application không implement Widevine cryptography trực tiếp.

Nó sử dụng EME để làm việc với CDM.

---

# 19. Initialization Data

CDM cần **Initialization Data** để tạo license request.

Với ISO-BMFF/Common Encryption, initialization data thường liên quan tới PSSH.

Browser flow:

```text
Encrypted Media
      |
      | contains initialization data
      v
Browser
      |
      | "encrypted" event
      v
Application
      |
      | initData
      v
CDM

```

Theo EME:

```text
encrypted event
      |
      v
event.initData
      |
      v
MediaKeySession.generateRequest(...)
      |
      v
CDM

```

---

# 20. MediaKeySession

Browser tạo một **MediaKeySession**.

Nó là context cho quá trình:

```text
Initialization Data
       |
       v
MediaKeySession
       |
       v
License Message
       |
       v
License
       |
       v
Keys available to CDM

```

Một session có thể chứa nhiều keys, mỗi key gắn với một KID.

---

# 21. CDM tạo License Challenge

CDM nhận initialization data:

```text
PSSH / Init Data
       |
       v
      CDM
       |
       v
License Challenge

```

Challenge là DRM-specific.

Ví dụ:

```text
Widevine
    > Widevine License Request

PlayReady
    > PlayReady License Challenge

FairPlay
    > SPC

```

Application nhận message từ CDM và gửi nó tới License Service.

---

# 22. Authentication & Entitlement

Trước khi xin DRM License, application phải xác định user có quyền xem content hay không. Đây là lớp **business authorization**, tách biệt với DRM challenge do CDM tạo.

```text
User
 |
 | login / access token
 v
Application Backend
 |
 +-- Authentication
 |      "User là ai?"
 |
 +-- Authorization / Entitlement
        "User có quyền xem content này không?"
 |
 v
Entitlement Service
 |
 +-- subscription / purchase / rental
 +-- content / KID permission
 +-- expiration
 +-- geo / concurrency rules
 +-- DRM usage policy
 |
 v
Signed Entitlement
```

Với Axinom, License Acquisition API không dựa vào HTTP `Authorization` header. Thay vào đó, License Request phải mang một **License Service Message** chứa **Entitlement Message hợp lệ**.

Một entitlement conceptually ràng buộc quyền với Content Key/KID và policy:

```text
Signed Entitlement
|
+-- authorized KID(s)
+-- validity / expiration
+-- license lifetime
+-- DRM usage policy
+-- optional device/IP restrictions
```

Với Axinom, Entitlement Message được wrap vào License Service Message và ký JWT bằng **HMAC-SHA256 + Communication Key**:

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

`Communication Key` là shared secret giữa trusted backend và Axinom License Service. Nó phải ở server side; nếu secret này bị ship xuống client, attacker có thể tự tạo entitlement hợp lệ.

Sau bước này Player có hai artifact độc lập:

```text
① DRM License Challenge
   +-- do CDM tạo
       +-- requested KID
       +-- device / DRM-specific information
       +-- cryptographically protected challenge

② Signed Entitlement
   +-- do trusted backend tạo
       +-- authorized KID(s)
       +-- validity
       +-- DRM policy
```

License Service chỉ cấp license khi hai phía khớp nhau. Ví dụ:

```text
CDM requests:       KID = A
Entitlement allows: KID = A

        v

     GRANT
```

nhưng:

```text
CDM requests:       KID = B
Entitlement allows: KID = A

        v

      DENY
```

Do đó **URL License Server + KID không phải authorization credential**.

---

# 23. License Acquisition

Player/Application đóng vai trò nối hai dữ liệu: challenge do CDM tạo và entitlement do trusted backend cấp.

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
                 validate entitlement
                 validate requested KID
                 validate DRM/device data
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

Với Axinom Standard Mode, signed License Service Message có thể được truyền qua `X-AxDRM-Message`, query parameter `AxDrmMessage`, hoặc PlayReady Custom Data tùy integration.

**CPIX không xuất hiện ở đây.** CPIX thuộc packaging side; entitlement + DRM challenge thuộc playback/license side.

---

# 24. License Service

License Service không đơn giản nhận KID rồi trả key. Nó phải validate **authorization artifact** và **DRM request** trước khi tạo license.

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
Validate DRM/device/security policy
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

Ví dụ:

```text
KID = UUID-A
      |
      v
License Service
      |
      v
Content Key = SECRET-A
```

`SECRET-A` phải là cùng key đã được Packager sử dụng. License Service đưa key vào DRM-specific license ở dạng được bảo vệ; application không nhận plaintext key.

License challenge cũng là DRM-specific và thường mang device identification / DRM-specific protected data. DRM License trả về được ràng buộc với client/CDM đã tạo request theo cơ chế của từng DRM system.

---

# 25. DRM License bảo vệ Content Key

License Service **không trả plaintext Content Key cho application**.

Conceptually:

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

License có thể bao gồm:

```text
Content Key
Rights
Expiration
Output Restrictions
Other DRM Policy

```

Exact cryptographic format phụ thuộc từng DRM system.

---

# 26. CDM nhận License

Application đưa license response vào CDM.

Trên EME:

```text
License Response
       |
       v
MediaKeySession.update(...)
       |
       v
      CDM

```

CDM xử lý license và làm key khả dụng cho session.

Conceptually:

```text
CDM Key Store / Session

KID UUID-A
     |
     +-- Content Key SECRET-A

```

Application không cần — và thông thường không được — nhìn thấy plaintext key.

---

# 27. Decrypt Media Sample

Player tiếp tục tải encrypted CMAF fragments:

```text
segment_001.m4s
segment_002.m4s
...

```

Một encrypted sample có:

```text
Encrypted Sample
|
+-- KID / key association
+-- IV
+-- encryption scheme
+-- subsample information
+-- ciphertext

```

Decryptor/CDM xác định:

```text
KID = UUID-A
      |
      v
Find Key
      |
      v
SECRET-A

```

Sau đó:

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

Đây chính là phép toán ngược với packaging side.

---

# 28. Decoder

Sau DRM decryption, dữ liệu chưa phải raw video frame.

Nó vẫn là **compressed media**:

```text
Encrypted H.264 Sample
        |
        | DRM decrypt
        v
Clear H.264 Sample
        |
        | H.264 decoder
        v
Raw Video Frame

```

Tương tự:

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
Video Frame

```

Do đó cần phân biệt:

```text
DECRYPT
    ≠
DECODE

```

Decrypt:

```text
ciphertext > clear compressed media

```

Decode:

```text
compressed media > raw video/audio

```

---

# 29. Secure Playback Path

Ở DRM security level cao, Content Key và clear media không nhất thiết được expose cho normal application memory.

Conceptually:

```text
              Untrusted World
----------------------------------------

Application
Browser
Network
Encrypted Media


              Security Boundary
----------------------------------------

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
                  Display

```

Implementation cụ thể phụ thuộc DRM, OS và hardware.

Có thể có:

```text
TEE
Trusted Execution Environment

Hardware-backed key storage

Secure Decoder

Protected Video Path

HDCP

```

Mục tiêu là hạn chế việc plaintext key hoặc decoded high-value content xuất hiện ở vùng memory mà application thông thường có thể truy cập.

---

# 30. Software DRM vs Hardware-backed DRM

Conceptually:

```text
Software DRM

Encrypted Media
      |
      v
Software CDM
      |
      v
Software Decryption
      |
      v
Decoder

```

Hardware-backed:

```text
Encrypted Media
      |
      v
CDM / Trusted Environment
      |
      v
Hardware-backed Key Handling
      |
      v
Secure Decryption
      |
      v
Secure Decoder
      |
      v
Protected Output

```

Exact security architecture phụ thuộc platform và DRM implementation.

---

# 31. Key Service và License Service phải đồng bộ

Đây là invariant quan trọng nhất của DRM backend.

Packaging:

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

Playback:

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

Nếu:

```text
Encryption Key = SECRET-A

Decryption Key = SECRET-B

```

thì:

```text
Ciphertext
    +
SECRET-B
    |
    v
INVALID DATA

```

Playback thất bại.

---

# 32. Key Rotation

Không bắt buộc một stream phải dùng một Content Key mãi mãi.

Ví dụ live stream:

```text
TIME ----------------------------------------->

Period 1          Period 2          Period 3

KID-A             KID-B             KID-C
KEY-A             KEY-B             KEY-C

segment 1-10      segment 11-20     segment 21-30

```

Player/CDM phải acquire và sử dụng đúng key tương ứng với từng period.

CPIX hỗ trợ mô hình key periods cho content preparation.

---

# 33. Multi-DRM

Multi-DRM không nhất thiết có nghĩa:

```text
Widevine
    > encrypted copy A

PlayReady
    > encrypted copy B

FairPlay
    > encrypted copy C

```

Common Encryption cho phép:

```text
                  Encrypted CMAF
                        |
           +------------+------------+
           v            v            v
       Widevine     PlayReady     FairPlay

```

nếu các DRM và client target cùng hỗ trợ encryption scheme được chọn.

Sự khác biệt chủ yếu nằm ở:

```text
DRM signaling

License protocol

License format

CDM

Security level

Policy enforcement

```

chứ không nhất thiết ở ciphertext.

---

# 34. Security Boundaries

Có thể chia dữ liệu thành ba nhóm.

### Public / distributable

```text
Encrypted CMAF
Manifest
KID
PSSH
DRM signaling

```

### Secret

```text
Content Key
Key Seed
Provider credentials
Signing secrets

```

### DRM-protected

```text
DRM License
Device credentials
CDM state
Protected key material

```

Content Key không nên:

```text
log ra console

ghi plaintext vào disk nếu không cần

đưa vào metrics

đưa vào error message

expose qua application API

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

Nếu chỉ nhớ một flow:

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

Hay ngắn nhất:

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

Đây là xương sống của toàn bộ DRM system.

**Key Service giúp Encryptor có Content Key ở packaging time.**

**Entitlement Service quyết định user/client có quyền xin license cho content/KID nào.**

**License Service chỉ phát DRM License sau khi authorization hợp lệ và DRM request thỏa policy.**

**License Service giúp CDM có cùng Content Key ở playback time, nhưng ở dạng được DRM bảo vệ.**

**CPIX/SPEKE giải quyết key exchange phía packaging.**

**Signed Entitlement + DRM License Challenge giải quyết authorization/license acquisition phía playback.**

**Widevine / PlayReady / FairPlay giải quyết license format, key protection, device trust và policy enforcement phía playback.**

**Common Encryption (`cenc` / `cbcs`) định nghĩa cách media được encrypt/decrypt.**

**CDM giữ và sử dụng key để decrypt media trên thiết bị.**

---

# 37. Tài liệu tham khảo

- DASH-IF CPIX: https://dashif.org/CPIX/
- W3C Encrypted Media Extensions: https://www.w3.org/TR/encrypted-media/
- Axinom DRM License Service: https://docs.axinom.com/services/drm/license-service/
- Axinom License Acquisition API: https://docs.axinom.com/services/drm/license-service/license-acquisition-api
- Axinom — Sign License Service Message: https://docs.axinom.com/services/drm/how-to-guides/sign-license-service-message
- Axinom — What is DRM?: https://docs.axinom.com/services/drm/general/what-is-drm/
