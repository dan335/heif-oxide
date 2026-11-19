# Changelog

## 0.1.1 — 2026-11-18

### Fixed

- Files without a `colr` nclx box now take sample range, matrix and
  primaries from the HEVC bitstream (SPS VUI) instead of assuming
  limited-range BT.709. iPhone photos, which carry only an ICC profile,
  were decoding with crushed shadows, clipped highlights and no Display P3
  to sRGB conversion. A container nclx still takes precedence. (#3)
- `imir` mirror axis: axis 0 exchanges top and bottom, axis 1 exchanges
  left and right, matching libheif. The two were swapped. (#6)

### Added

- `read_metadata`: dimensions, bit depth, alpha presence and colour
  information without decoding pixels. (#5)
- `wasm32` targets decode on a single thread instead of trying to spawn
  threads. (#4)

### Changed

- `rust_h265` updated from 0.1 to 0.4.1. The minimum supported Rust version
  is now 1.88. 4:4:4 streams are rejected with a clear error. (#8)
- The Display P3 / BT.2020 to sRGB conversion uses interpolated lookup
  tables for the sRGB curves.

Pixel output changes for every file affected by the two fixes.

## 0.1.0 — 2026-07-27

Initial release.
