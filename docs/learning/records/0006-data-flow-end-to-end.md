# Learning Record 0006 — Data Flow End-to-End

**Date:** 2026-09-08
**Lesson:** [0003-data-flow-end-to-end](../lessons/0003-data-flow-end-to-end.html)

## Key Insight

drmpack là **orchestrator**, không phải encryptor. Nó lấy key → tạo XML config → spawn GPAC → thu hoạch output. GPAC cecrypt filter làm mã hóa thật sự.

## Non-obvious Lessons

1. **Fan-out architecture**: Dual mode spawn 2 GPAC process song song cho CENC/CBCS, cùng nhận chung 1 luồng media input.
2. **Manifest-Driven Readiness**: Harvester không đọc segment khi thấy file trên disk — chỉ đọc khi HLS manifest xác nhận segment hoàn chỉnh. DASH MPD không dùng được cho mục đích này vì SegmentTemplate có mặt từ đầu session.
3. **Ephemeral Staging**: File bị xoá ngay sau khi đọc vào memory → tiết kiệm disk/ramdisk.
4. **Init segment bypass**: Init segment nhỏ, GPAC ghi atomic → không cần đợi manifest.

## Zone of Proximal Development

Tiếp theo nên học:
- Chi tiết CPIX/SPEKE key exchange protocol
- GPAC filter chain configuration options
- ISO-BMFF box structure (ftyp, moov, moof, mdat)
