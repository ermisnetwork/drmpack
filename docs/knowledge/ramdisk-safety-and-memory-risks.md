# Deep-Dive Research Report: Safety, Memory Risks, and Protective Architecture When Writing Video Packaging Output to Ramdisk / tmpfs in Production

**Target Document:** `docs/knowledge/ramdisk-safety-and-memory-risks.md`  
**Project:** `drmpack` (Ermis Stream Multi-DRM Packaging Engine)  
**Status:** Completed research from Primary Sources  
**Date:** 2026-09-08  

---

## 1. Executive Summary & Context

In the initial architecture of `drmpack` ([ADR-0005](../adr/0005-ramdisk-tmpfs-manifest-distribution.md)), Ramdisk (`/dev/shm` on Linux or `tmpfs`) was chosen as the storage destination for packaging output (HLS `.m3u8`, DASH `.mpd`, and CMAF `.m4s` media segments). This decision aimed to eliminate disk I/O latency and prevent physical disk wear caused by high-frequency manifest overwrite cycles (200ms – 2s) in Low-Latency Live streams.

However, deploying this solution in **large-scale production environments (Linux bare-metal, Docker containers, Kubernetes Pods)** exposes **5 critical potential hazards** if not carefully managed:

1. **The 64MB Default Container Trap:** Docker and Kubernetes (CRI-O / containerd) default to allocating exactly **64 MiB** for `/dev/shm`. With a multi-bitrate 1080p-720p-360p stream, 64MB fills up in just **28 to 56 seconds**, causing GPAC to crash with an `ENOSPC` (No space left on device) write error and bringing down the entire live session.
2. **The 30-Minute Time-Shift Buffer (`tsb=1800`) Consumes Gigabytes of RAM:** Currently `src/gpac/process.rs:128` hardcodes the flag `tsb=1800` (30-minute Time-Shift Buffer). Although GPAC has a mechanism to auto-delete old segments once outside the window (`keep_segs=false`), for the first 30 minutes GPAC **does not delete any segments at all**. A multi-bitrate CENC + CBCS (Dual Scheme) stream accumulates up to **4.1 GB** of data in RAM. Running just 5 concurrent streams on a single node exceeds 20 GB of RAM consumption!
3. **Cgroup Double-Accounting & OOM Killer (Exit Code 137):** In Linux cgroup v2 and Kubernetes, `tmpfs` memory is counted directly toward the container's memory limit (`memory.current` / `resources.limits.memory`). If an engineer configures `emptyDir.medium: Memory` with a 4GB size limit but sets the container limit to 2GB, the Kernel OOM Killer sends a `SIGKILL` to terminate the process immediately.
4. **Permanent Leakage on Abrupt Process Termination (Zombie / Orphaned Files):** Unlike anonymous process memory which is automatically reclaimed upon process exit, `tmpfs` is a kernel-managed Virtual Filesystem (VFS) mount point. When a process suffers a `SIGKILL`, crash, segfault, or OOM, RAII cleanup mechanisms (`Drop` in Rust) **do not run at all**. Gigabytes of media segments remain trapped in host RAM. Under a Kubernetes `CrashLoopBackOff` scenario, host node RAM is exhausted after just a few restart cycles.
5. **Misconceptions Regarding NVMe SSD Speed and the Linux Page Cache:** Modern systems performance analysis demonstrates that a `write(2)` call to an SSD actually writes into the kernel's **Linux Page Cache in RAM** with nanosecond latency on par with `tmpfs`. SSDs introduce no I/O bottleneck for media segments (200KB – 2MB). Crucially: **Page Cache on SSD can be evicted and flushed to disk automatically under memory pressure**, acting as a safety valve that eliminates OOM Killer crashes, an advantage that unswapped `tmpfs` completely lacks.

---

## 2. Deep-Dive Investigation of the 5 Core Risks from Primary Sources

### 2.1 tmpfs and /dev/shm Mechanics in Linux Kernel & OOM Killer Behavior

#### Primary Sources
* Linux Kernel Documentation: `Documentation/filesystems/tmpfs.rst`
* Linux Kernel Source: `mm/shmem.c`, `mm/oom_kill.c`, `mm/memcontrol.c`
* Linux Programmer's Manual: `tmpfs(5)`, `shm_overview(7)`, `cgroups(7)`

#### Page Allocation and Management Mechanics
According to Linux Kernel Documentation (`tmpfs.rst`), `tmpfs` is a filesystem that stores all files directly in kernel Virtual Memory, specifically the **Page Cache** and **dentry cache**:
* **Dynamic Allocation:** `tmpfs` does not preallocate its maximum capacity. Actual space consumed equals the size of currently stored files plus metadata (inodes).
* **Default Size:** When mounted without an explicit `size` parameter, the kernel defaults `tmpfs` capacity to **50% of the host's physical RAM** (`size=50%`).
* **Interaction with Swap:** `tmpfs` pages are managed as file-backed anonymous shared memory (`shmem`). Under memory pressure, the page-reclaim subsystem (`kswapd`) **CAN swap out `tmpfs` pages to physical Swap partitions**. However, in modern containerized infrastructure (especially Kubernetes), Swap is typically disabled (`swapoff -a`) per standard Kubernetes recommendations. Without Swap, all `tmpfs` pages are **pinned directly in physical RAM**.

#### Distinction: Disk Full (`ENOSPC`) vs. Memory Exhaustion (`OOM Killer`)
The kernel handles memory depletion across two distinct mechanisms:

```
                          ┌──────────────────────────┐
                          │ File write call: write(2)│
                          └─────────────┬────────────┘
                                        │
                 ┌──────────────────────┴──────────────────────┐
                 ▼                                             ▼
       [tmpfs hits size limit]                   [RAM / cgroup memory hits limit]
                 │                                             │
      VFS returns: -ENOSPC                           Kernel cannot allocate page
    "No space left on device"                                  │
                 │                               ┌─────────────┴─────────────┐
        Process receives I/O error               ▼                           ▼
        (OOM Killer DOES NOT run)           [With Swap]                 [No Swap]
                 │                               │                           │
    GPAC exits with I/O error code          Swap out tmpfs             Kernel cgroup triggers
                                                                     mem_cgroup_out_of_memory()
                                                                             │
                                                                    OOM Killer selects victim
                                                                             │
                                                                    Sends SIGKILL (Exit 137)
```

1. **Case A — Exceeding tmpfs `size` limit (`ENOSPC`):**
   * If `/dev/shm` is assigned a limit (e.g. 64MB on Docker) and written data exceeds 64MB, the VFS subsystem rejects the `write(2)` call and returns **`-ENOSPC` (No space left on device)**.
   * **Kernel Behavior:** The kernel **DOES NOT** invoke the OOM Killer. The error is returned to the calling process. If the application (GPAC) does not handle the write error, it terminates with an I/O error.
2. **Case B — Exhausting System RAM or Exceeding Container `memory.max` (OOM Killer):**
   * If `tmpfs` has a large limit (or no limit), and data written to `tmpfs` exhausts the host's physical RAM or exceeds the container's `memory.max` (cgroup v2) / `memory.limit_in_bytes` (cgroup v1):
   * In cgroup v2, `tmpfs` memory pages created by container processes are charged directly to `shmem` and `file` accounting under `memory.current`.
   * When `memory.current` hits the `memory.max` ceiling, the memory controller (`mem_cgroup`) begins reclaim cycles. Because `tmpfs` pages are dirty file pages with **no physical backing block device to flush to**, and Swap is disabled, the kernel is completely unable to reclaim memory!
   * The `mem_cgroup_out_of_memory()` function in `mm/memcontrol.c` is triggered. The `oom_badness()` heuristic scores the process consuming the most memory (typically GPAC or the media server) and dispatches an uncatchable **`SIGKILL` (signal 9)**. The container stops abruptly with **Exit Code 137 (`128 + 9`)**.

---

### 2.2 Risks in Container Environments (Docker & Kubernetes)

#### Primary Sources
* Docker Run Reference: Command-line reference (`--shm-size`, `/etc/docker/daemon.json`)
* Kubernetes Documentation: *Volumes: emptyDir*, *Assign Memory Resources to Containers and Pods*
* OCI Runtime Specification / runc implementation

#### The 64MB Docker Engine Trap
* By default, when creating any container using `docker run`, Docker Engine mounts a `tmpfs` filesystem at `/dev/shm` with a fixed size of **67,108,864 bytes (exactly 64 MiB)**.
* This 64MB default originates from historical security considerations (preventing an unprivileged process from exhausting host POSIX shared memory).
* **Impact on video streaming:** A multi-bitrate 1080p stream averages ~1.15 MB/s. After just **56 seconds** (single scheme) or **28 seconds** (Dual scheme CENC + CBCS), the 64MB space is 100% exhausted. GPAC immediately encounters an error:
  ```text
  [dasher] Error writing segment to /dev/shm/drmpack_live_123/cenc/video_1080p_15.m4s: No space left on device
  ```
  The GPAC process dies, the Unix pipe breaks (`BrokenPipe`), and `PackagingSession` crashes completely.
* **Manual Remediation:** Operators must explicitly pass `--shm-size=2gb` in `docker run` or configure `shm_size: 2gb` in `docker-compose.yml`.

#### Configuration Pitfalls on Kubernetes (K8s Pods)
By default, Pods running on Kubernetes inherit runtime defaults from containerd/CRI-O and also receive only **64 MiB** in `/dev/shm`. To expand it, engineers must mount an `emptyDir` volume with `medium: Memory`:

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: video-packager
spec:
  containers:
  - name: packager
    image: ermis/drmpack-service:latest
    resources:
      limits:
        memory: "2Gi" # PITFALL 1: Container cgroup limit
    volumeMounts:
    - mountPath: /dev/shm
      name: shm-volume
  volumes:
  - name: shm-volume
    emptyDir:
      medium: Memory
      sizeLimit: "4Gi" # PITFALL 2: Volume sizeLimit
```

Three critical pitfalls exist in this model:
1. **Cgroup Double-Accounting Trap:**
   * Data written to `emptyDir.medium: Memory` is billed directly to the Pod/Container memory usage.
   * In the example above, the engineer configured `sizeLimit: 4Gi` for `/dev/shm`, but set `resources.limits.memory` to `2Gi`. When video buffering accumulates to ~1.8GB (plus 200MB process RSS), the **Pod is immediately OOMKilled by the kernel**, even though `/dev/shm` has used less than 50% of its volume `sizeLimit`!
2. **Kubelet Eviction Trap:**
   * Kubelet runs periodic volume usage monitors. If `/dev/shm` exceeds `sizeLimit`, Kubelet flags the Pod as violating resource limits and proceeds to **Evict the Pod** (`PodTheNodeWasLowOnResource`).
3. **Missing `sizeLimit` Trap (Host Starvation):**
   * Declaring `emptyDir: { medium: Memory }` without setting `sizeLimit` allows the volume to expand up to the memory limit of the physical node. Under a memory leak or extended live session, the Pod can consume tens of gigabytes of node RAM, triggering `NodeMemoryPressure` and destabilizing co-located pods.

---

### 2.3 GPAC dasher Segment Deletion Behavior & Quantitative Capacity Analysis

#### Primary Sources
* GPAC Documentation: Filter `dasher` parameters (`gpac -h dasher`)
* GPAC Source Code: `src/filters/dasher.c` (`dasher_del_segment`, `keep_segs`, `tsb` logic)

#### Mechanics of `tsb` and `keep_segs` Flags
In GPAC dasher, segment file lifecycles are governed by two parameters:
1. **`tsb` (Time-Shift Buffer - floating point, GPAC default: 30 seconds):** Specifies the maximum temporal depth of the DVR sliding window in the DASH/HLS manifest.
2. **`keep_segs` (boolean, default: `false`):**
   * When `keep_segs=false` (default in GPAC and in `drmpack`): GPAC **AUTOMATICALLY DELETES** physical segment files on disk/RAM when a segment's presentation timestamp falls outside the window:  
     $$\text{segment\_start\_time} < \text{current\_playback\_time} - \text{tsb}$$
   * When `keep_segs=true`: GPAC preserves all segments from stream onset until termination (used for VOD archiving).

#### The 30-Minute Time Bomb of `tsb=1800`
In the current codebase of `drmpack` ([`src/gpac/process.rs:128`](../../src/gpac/process.rs#L128)):
```rust
"{}:dual:profile=live:dmode=dynauto:segdur={}:spd={}:tsb=1800:utcs=inband:pssh=mv:template=$RepresentationID$_$Init=init$$Number$"
```
The parameter `tsb=1800` (1800 seconds = 30 minutes) leads to:
* **Linear accumulation phase (0 to 30 minutes):** For the first 1800 seconds of a stream, **GPAC DOES NOT DELETE A SINGLE BYTE**! Media segments from all renditions and schemes continuously pile up in `/dev/shm`.
* **Saturation phase (After minute 30):** Only from second 1801 onward does GPAC begin deleting segment 1 as new segments arrive, stabilizing RAM consumption into a steady state.

#### Quantitative Ramdisk Capacity Analysis Table

Assuming a standard industrial multi-bitrate ABR ladder:
* **Rendition 1080p60:** 5,000 kbps (~625 KB/s)
* **Rendition 720p30:** 2,500 kbps (~312.5 KB/s)
* **Rendition 360p30:** 800 kbps (~100 KB/s)
* **Stereo AAC Audio:** 128 kbps (~16 KB/s)
* **Subtitles & Metadata:** 32 kbps (~4 KB/s)
* **CMAF Packaging & Filesystem Inode Overhead:** 8% (encompassing `moof`, `traf`, `tfhd`, `trun`, `sidx`, `pssh` boxes, `senc`/`saiz`/`saio` metadata, and filesystem block allocation overhead).

*Total actual bandwidth (Single Scheme):* $8,460\text{ kbps} \times 1.08 \approx 9,136\text{ kbps} \approx 1.142\text{ MB/s}$  
*Total actual bandwidth (Dual Scheme CENC + CBCS):* $1.142\text{ MB/s} \times 2 \approx 2.284\text{ MB/s}$

| Buffer Window (`tsb`) | Single Scheme (`MB` / `GiB`) | Dual Scheme CENC+CBCS (`MB` / `GiB`) | 5 Concurrent Streams (Dual) | 10 Concurrent Streams (Dual) | Safety Assessment |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **30 seconds** (`tsb=30`) | 34.3 MB (0.03 GiB) | **68.5 MB (0.07 GiB)** | 342.5 MB | 685 MB | **Very safe**, fits small RAM budgets |
| **60 seconds** (`tsb=60`) | 68.5 MB (0.07 GiB) | **137.0 MB (0.13 GiB)** | 685 MB | 1.37 GiB | **Optimal for Live Edge / Low Latency** |
| **120 seconds** (2 min) | 137.0 MB (0.13 GiB) | **274.1 MB (0.26 GiB)** | 1.37 GiB | 2.74 GiB | Safe for short DVR |
| **300 seconds** (5 min) | 342.6 MB (0.32 GiB) | **685.2 MB (0.64 GiB)** | 3.43 GiB | 6.85 GiB | Requires minimum 1GB/stream |
| **1800 seconds** (30 min - Current default) | **2.05 GB (1.91 GiB)** | **4.11 GB (3.83 GiB)** | **20.55 GB (19.1 GiB)** | **41.1 GB (38.3 GiB)** | **CRITICALLY DANGEROUS: High OOM Risk** |
| **64MB Docker Limit** | **Disk full in 56s!** | **Disk full in 28s!** | Immediate crash | Immediate crash | **100% Guaranteed failure** |

---

### 2.4 Resource Leakage & Zombie/Orphaned Files

#### Primary Sources
* Linux Programmer's Manual: `shm_overview(7)`
* POSIX Standard IEEE Std 1003.1 (Shared Memory Objects Lifecycle)
* Rust Standard Library Documentation: `std::ops::Drop` guarantees and caveats

#### Lifecycle Nature of tmpfs vs Anonymous Memory
* When a process allocates normal RAM memory via `malloc` or `mmap(MAP_ANONYMOUS)`, the kernel tracks that memory page via the process's page table structure `mm_struct`. When the process terminates (whether cleanly or killed by `SIGKILL`), the kernel **always reclaims 100% of anonymous memory** in `exit_mmap()`.
* **However, `tmpfs` and `/dev/shm` ARE NOT process memory!** `tmpfs` is a kernel Virtual Filesystem (VFS) mount point. Every file created in `/dev/shm` exists independently of the process that wrote it.
* **POSIX & Linux VFS Rules:** Files in `tmpfs` are only freed when:
  1. A process explicitly calls `unlink(2)` / `remove(3)` to delete the file.
  2. The entire `tmpfs` filesystem is unmounted (`umount`).
  3. The host reboots.

#### Severe Limitations of RAII (`Drop` in Rust)
In `drmpack`, we have a directory cleanup destructor:
```rust
// src/session/mod.rs:778
impl Drop for PackagingSession {
    fn drop(&mut self) {
        // Deletes output_dir and control_dir
    }
}
```
**Fatal limitations of Rust `Drop`:**
* The Rust runtime only executes `Drop::drop` when a struct exits scope normally or during **stack unwinding due to `panic!`**.
* `Drop` **DOES NOT RUN AT ALL** under:
  * Process receiving `SIGKILL` (signal 9) from the Kernel OOM Killer, Docker daemon (`docker stop` after timeout), or `kill -9`.
  * Segmentation Fault (`SIGSEGV`), Bus Error (`SIGBUS`).
  * Direct invocation of `std::process::exit()` or `libc::_exit()`.
  * Child GPAC process crash causing supervisor hang or crash.

#### The CrashLoopBackOff Crisis Scenario
When a container running `drmpack` is OOMKilled or panics:
1. The directory `/dev/shm/drmpack_{content_id}_{uuid}` holding **~4GB** of video segments is **completely orphaned** in RAM.
2. The container runtime restarts a new Pod.
3. The new Pod generates a new random UUID: `/dev/shm/drmpack_{content_id}_{new_uuid}` and continues writing another 4GB into RAM.
4. After several restart cycles within a few minutes, host physical RAM is exhausted, turning the node into a zombie that cannot accept new connections!

---

### 2.5 Practical Comparison: NVMe SSD + Linux Page Cache vs RAM (/dev/shm)

#### Primary Sources
* Linux Kernel Documentation: `Documentation/admin-guide/sysctl/vm.rst` (`dirty_background_ratio`, `dirty_ratio`, `dirty_expire_centisecs`)
* Brendan Gregg: *Systems Performance: Enterprise and the Cloud* (2nd Edition, Chapter 8: File Systems)
* Enterprise NVMe SSD Datasheets (Samsung PM9A3, Solidigm D7-P5520, Intel Optane)

#### 1. Linux Page Cache Mechanics (Write Latency is Equivalent)
When an application calls `write(2)` on a file located on a standard SSD filesystem (ext4, xfs):
* The write call **DOES NOT wait for data to commit to SSD flash chips**! The write call is simply a memory copy operation from user-space buffer into the kernel's **Page Cache (RAM)**.
* The operating system returns success to the application immediately within **several hundred nanoseconds**, matching the speed of writing to `/dev/shm`.
* Flushing data from Page Cache down to physical disk is handled asynchronously by background kernel threads (`wb_workfn` / `kworker`).
* When the HTTP server (Origin / CDN Edge) reads the newly written segment file to serve viewers, `read(2)` hits the **Page Cache already hot in RAM** (Page Cache Hit Rate $\approx 100\%$). The physical SSD performs almost no read operations!

#### 2. Flash Endurance & Wear (SSD Flash Endurance)
* The primary concern with SSDs is flash wear (Write Amplification) caused by continuous write cycles.
* **For Media Segments (`.m4s` sizing 200KB – 2MB):** These are large sequential writes aligned with NAND flash block sizes. The Write Amplification Factor (WAF) approximates 1.0.
  * A 10 Mbps stream generates: $1.25\text{ MB/s} = 108\text{ GB/day}$.
  * An Enterprise NVMe SSD (e.g., 1.92TB @ 1 DWPD) allows **1,920 GB/day continuously for 5 years**.
  * One live stream consumes only **5.6%** of daily drive endurance. Even 10 concurrent streams running 24/7 remain durable for years.
* **For Manifests (`.m3u8`, `.mpd` sizing a few KB):** These small files are overwritten 5 to 10 times per second in Low Latency mode. Small random overwrites cause fragmentation and force SSD garbage collection cycles, increasing latency spikes.

#### 3. NVMe SSD Acts as a "Safety Valve" Eliminating OOM Risks
* Under system memory pressure, dirty SSD pages **can be flushed to physical disk and freed from RAM immediately**!
* SSD Page Cache is reclaimable memory. Conversely, `tmpfs` pages (without swap) are completely unreclaimable.
* **Conclusion:** Using SSDs for segment buffering converts an **OOM Crash (total system outage)** into **Transient I/O Latency (system stays alive)**.

---

## 3. Recommended Defensive Architecture for `drmpack`

1. **Reduce default `tsb` from 1800s to 60s (or 30s):**
   * Immediately slashes 97% of RAM consumption (from 4.1 GB down to ~137 MB for Dual scheme).
   * Add `.with_time_shift_buffer(Duration)` builder method.
2. **Pre-flight Check Mechanism:**
   * Detect if output directory resides on a `tmpfs` mount with $\le 64\text{MB}$ capacity (Docker trap) and warn or reject early before GPAC crashes.
3. **Active Orphan Session Reaper:**
   * Provide a `PackagingSession::reap_orphaned_sessions()` function to clean up zombie RAM directories on process/container restart.
4. **Flexible Storage Configuration:**
   * Allow configuring `output_dir` to an NVMe SSD path for workloads requiring long buffer windows without OOM vulnerability.
