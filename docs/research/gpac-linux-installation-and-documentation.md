# Research: GPAC Linux Installation Strategies & Documentation Optimization

- **Author / System**: `drmpack` Research Subsystem
- **Date**: 2026-09-18
- **Status**: Completed (Primary Sources Verified)
- **Objective**: Re-evaluate GPAC installation mechanisms on Linux, analyze the root causes of failure in hardcoded distribution scripts, identify authoritative upstream GPAC primary documentation links, and define an optimal, lean documentation pattern for `README.md`.

---

## 1. Primary Sources & Authoritative Resources

Through direct inspection of upstream GPAC and Linux distribution packaging ecosystems, the verified primary resources are:

| Resource | Official / Canonical URL | Role & Characteristics |
| :--- | :--- | :--- |
| **GPAC Project Home** | [https://gpac.io/](https://gpac.io/) | Project portal, feature overview, and announcements. |
| **Official Downloads Portal** | [https://gpac.io/downloads/](https://gpac.io/downloads/) | Central hub for prebuilt packages, release links, and nightly installers. Note: Protected by Cloudflare/rate limiting (returns HTTP 429 to automated scrapers). |
| **Official Wiki & Technical Docs** | [https://wiki.gpac.io/](https://wiki.gpac.io/) | Authoritative reference for filters (`cecrypt`, `dasher`), filter graph syntax, and core architecture. |
| **Official Linux Build Guide** | [https://wiki.gpac.io/Build/GPAC-Build-Guide-for-Linux/](https://wiki.gpac.io/Build/GPAC-Build-Guide-for-Linux/) | Core developer documentation for compiling from source using GNU Autotools (`./configure && make`). |
| **GitHub Source Repository** | [https://github.com/gpac/gpac](https://github.com/gpac/gpac) | Source repository, issue tracker, commit tree, and pull requests. |
| **GitHub Releases & Tags** | [https://github.com/gpac/gpac/releases](https://github.com/gpac/gpac/releases) | Official release tags (`v2.2.1`, `v26.07.0`), source tarballs. |
| **Official Docker Hub Image** | [https://hub.docker.com/r/gpac/gpac](https://hub.docker.com/r/gpac/gpac) (`gpac/gpac`) | Official container images provided and maintained by the GPAC team. |
| **GPAC Nightly APT Mirror** | [https://dist.gpac.io/](https://dist.gpac.io/) | Standalone `.deb` host maintained by the GPAC team; single-server infrastructure subject to intermittent network timeouts and latency. |

---

## 2. Analysis of Linux Installation Approaches

### 2.1. Distribution Native Package Managers
- **Ubuntu 24.04 LTS (`noble`)**:
  - `gpac` is available directly in the standard `universe` repository.
  - Version: `>= 2.2.1` (fully meets `drmpack` requirements).
  - Command: `sudo apt-get update && sudo apt-get install -y gpac`.
- **Ubuntu 22.04 LTS (`jammy`)**:
  - Default package repository provides only `gpac 2.0.0` (too old, lacks required filter flags for `cecrypt` / `dasher`).
- **Debian 12 (`bookworm`)**:
  - `gpac` was removed from stable `main` repositories due to maintenance/CVE cycles. Only available in Debian testing (`trixie`) or unstable (`sid`).
- **Arch Linux**: Latest version available in `extra` repository (`sudo pacman -S gpac`).
- **Fedora / RHEL / CentOS**: Available via RPM Fusion repository (`sudo dnf install gpac`).
- **Alpine Linux**: No native package in `apk`. Requires building from source or using multi-stage Docker builds.

### 2.2. GPAC APT Repository (`dist.gpac.io`)
- **Mechanism**: Adding GPG key from `https://dist.gpac.io/gpac/linux/gpg.asc` and repository sources for Ubuntu.
- **Real-world Pitfalls**:
  1. **Infrastructure Flakiness**: `dist.gpac.io` experiences frequent timeouts and connection drops. In this repository's own CI workflow (`.github/workflows/drm-e2e.yml`), retries and an explicit fallback to `web.archive.org` were necessary:
     ```yaml
     sudo curl -fsSL --retry 3 --connect-timeout 10 https://dist.gpac.io/gpac/linux/gpg.asc -o /etc/apt/keyrings/gpac.asc || \
     sudo curl -fsSL https://web.archive.org/web/2024/https://dist.gpac.io/gpac/linux/gpg.asc -o /etc/apt/keyrings/gpac.asc
     ```
  2. **Derivative Distribution Incompatibility**: Commands using `$(. /etc/os-release && echo "$ID")` generate URLs like `https://dist.gpac.io/gpac/linux/linuxmint` or `pop`, resulting in immediate `404 Not Found` errors.
  3. **GPG Key Management Evolution**: Recent Ubuntu and Debian releases transitioned from `apt-key` to `/etc/apt/keyrings/` and deb822 format (`.sources`), leading to configuration warnings or permission failures when pasting outdated snippet variants.

### 2.3. Compiling From Source
- **Characteristics**: Distribution-independent, links against native host OpenSSL and libc, and functions on any architecture (x86_64, aarch64 / Apple Silicon / AWS Graviton).
- **Build System**: GPAC uses **GNU Autotools** (`./configure` and `make`), **not CMake** (documented in `docs/research/github-actions-ci-failures-analysis.md`).
- **Standard Workflow**:
  ```bash
  sudo apt-get update && sudo apt-get install -y build-essential git pkg-config zlib1g-dev libssl-dev
  git clone https://github.com/gpac/gpac.git
  cd gpac
  git checkout v26.07.0  # Or tag >= v2.2.1
  ./configure --prefix=/usr/local --use-ffmpeg=no
  make -j$(nproc)
  sudo make install
  sudo ldconfig
  ```
- **Trade-off**: Requires a C/C++ compiler toolchain and takes 2 to 4 minutes to compile.

### 2.4. Docker Container (Production Standard)
- Highly recommended for microservice deployments: Use official `gpac/gpac` or base images on Ubuntu 24.04 where `apt-get install gpac` is supported natively.

---

## 3. Evaluation of User Proposal

> *User Feedback: "I think we should only write generally about installing for Linux rather than writing out explicit step-by-step documentation in the README like that, because I noticed some methods work and some fail. Or linking to the official GPAC Linux installation guide would be more appropriate."*

### Conclusion: Technically Sound and Best Practice
Embedding monolithic, distribution-specific installation scripts in a Rust library's `README.md` introduces significant liabilities:
1. **Scope Boundary**: `drmpack` is a media packaging orchestration library, not an OS package distributor. Upstream dependencies should point to authoritative upstream guides.
2. **Documentation Rot**: Repository URLs, GPG keys, and distribution codenames drift over time. Broken installation scripts erode user confidence in the library itself.
3. **Fragmentation**: Linux environments vary widely. A script tuned solely for Ubuntu noble fails on Debian 12, Mint, Arch, or Alpine.

---

## 4. Implementation Strategy for `README.md`

Restructure the **System Prerequisites** section in `README.md` following a **Lean Contract-First** pattern:
1. State the interface contract (`gpac >= 2.2` in `PATH`, required filters `cecrypt`, `dasher`, `mp4dmx`).
2. Provide a 1-line verification check (`gpac -version && gpac -h cecrypt dasher`).
3. Link directly to authoritative GPAC resources (Downloads, Linux Build Guide, GitHub, Docker Hub).
4. Provide concise quick-start notes for Ubuntu 24.04 (`apt install gpac`), macOS (`brew install gpac`), and Docker containers.
