# Changelog

## Unreleased

### Breaking changes

- Replace `decode_bytes_with_limit(data, (max_width, max_height))` with
  `decode_bytes_with(data, &Limits)`. Size limits now count pixels rather
  than bounding each dimension separately. `decode_bytes` and `decode_file`
  use the default limits.
- Add non-exhaustive `Limits` with public `usize` fields: `max_pixels`
  (100,000,000), `max_grid_tiles` (1024), `max_item_payload_bytes` (256 MiB),
  and `max_picture_memory_bytes` (1 GiB). Construct with `Limits::default()`
  and modify fields; `Debug` and `Clone` are also implemented.
- Mark `HeifError` non-exhaustive. `ImageTooLarge` now has `width: u32`,
  `height: u32`, and `max_pixels: usize` fields instead of separate maximum
  width and height fields. Downstream error matches need a wildcard arm.

### Security

- Enforce pixel and picture-memory limits in the HEVC decoder using actual
  bitstream dimensions, including grid tiles and best-effort auxiliary
  alpha, rather than trusting container `ispe` declarations. HEVC limit
  violations may return `HeifError::Codec`.
- Bound declared image sizes before decoding, grid tile counts, aggregate
  tile pixels, canvas pixels, and assembled item payloads. Grid limits count
  actual aggregate tile pixels and the full canvas before cropping, even
  with understated `ispe` declarations. Invalid declared grid aggregates
  remain `HeifError::Invalid` errors.
- The picture-memory limit applies per HEVC picture, not to the total
  memory used by a decode or by concurrently decoded grid tiles.

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
