# Performance: the Rust/GPUI rewrite vs the Swift app

Loupe 0.10.0 replaced the SwiftUI/AppKit app (0.9.1) with a Rust app on GPUI. Both apps were measured on the same
Mac with the same data, as release builds, taking the median of at least 3 runs. The test data was a mock Forgejo
with three repositories, 38 pull requests and a working tree with a 150,000-line file change.

| Measure | Swift 0.9.1 | Rust/GPUI 0.10.0 |
|---|---|---|
| Click a 150k-line file → first full frame | 0.204 s | **0.164 s** |
| Scrolling that file (120 Hz display) | 106.7 fps, p95 16.8 ms | **116.7 fps**, p95 8.9 ms |
| Scroll start (wheel → first moved frame) | **52 ms** | 63 ms |
| Idle CPU per 60 s: Changes / History / an open PR | 0.00 / 0.01 / 0.05 s | 0.01 / 0.00 / 0.02 s |
| git processes while idle | 0 | 0 |
| 10 tab switches: CPU | 1.29 s | **0.15 s** |
| 10 tab switches: git processes | 0 | 0 |
| RSS after opening the 150k-line diff | 228 MB | **193 MB** |
| phys_footprint at that moment | **156 MB** | 231 MB |

Where the Rust app trails, the cause is inside GPUI, not Loupe:
- **Scroll start.** An idle window pauses its `CAMetalDisplayLink`. The first wheel event has to resume it and
  wait for the next frame.
- **phys_footprint.** GPUI keeps three full-window Metal drawables, about 34 MB each. Two would save about 34 MB,
  but the layer never times out waiting for a drawable, so a frame could stall the main thread.
