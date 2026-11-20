# heif-oxide

Pure-Rust HEIF/HEIC still-image decoder. No C dependencies, no `unsafe` in
this crate, MIT OR Apache-2.0. Requires Rust 1.88 or newer.

Parses the HEIF container (ISO/IEC 23008-12) and decodes the HEVC payload
with [`rust_h265`](https://crates.io/crates/rust_h265) 0.4 — a pure-Rust
HEVC decoder (its only `unsafe` is one documented module that supports its
multithreaded decoding). To our knowledge this is the first
permissively-licensed way to open iPhone photos in Rust without linking
libheif/libde265.

```rust
let image = heif_oxide::decode_file("photo.heic")?;
println!("{}x{}, {} channels, {}-bit",
    image.width, image.height, image.channels(), image.bit_depth());
let rgba: Vec<u8> = image.to_rgba8();          // or…
let f32s: Vec<f32> = image.to_f32_interleaved(); // 0.0..=1.0, native channels
```

Read dimensions, source bit depth, color information, and alpha presence without decoding pixels:

```rust
let data = std::fs::read("photo.heic")?;
let metadata = heif_oxide::read_metadata(&data)?;
println!("{}x{}, {}-bit", metadata.width, metadata.height, metadata.bit_depth);
```

## Decode limits

`decode_bytes` and `decode_file` use `Limits::default()`. Use
`decode_bytes_with(data: &[u8], limits: &Limits)` to customize the limits:

```rust
let data = std::fs::read("photo.heic")?;
let mut limits = heif_oxide::Limits::default();
limits.max_pixels = 24_000_000;
limits.max_grid_tiles = 256;
let image = heif_oxide::decode_bytes_with(&data, &limits)?;
```

`Limits` is non-exhaustive: construct it with `Default` and then set the
public fields rather than using a struct literal. It implements `Debug`
and `Clone`. All limits are `usize` values:

| Field | Default | Bounds |
| --- | --- | --- |
| `max_pixels` | `100_000_000` | Declared image size, actual HEVC pictures, grid tile aggregate and canvas. One HEVC picture is also capped at 35,651,584 pixels (the largest any HEVC level allows) |
| `max_grid_tiles` | `1024` | Number of tiles in a grid |
| `max_item_payload_bytes` | `256 * 1024 * 1024` (256 MiB) | Payload bytes assembled for one item |
| `max_picture_memory_bytes` | `512 * 1024 * 1024` (512 MiB) | HEVC decoder memory per picture |

Pixel limits are enforced against HEVC bitstream dimensions as well as
container declarations, so a small or spoofed `ispe` does not bypass them.
For grids, `max_pixels` counts the full, uncropped canvas and the aggregate
actual tile pixels, even when the declared output is much smaller.
The picture-memory limit is **per picture**, not a total budget for a decode:
parallel grid tiles, the assembled canvas, RGB output, and transforms can
consume additional memory. Alpha is best-effort; an auxiliary image that
fails decoding or exceeds its limits may be omitted.

Declared image sizes above `max_pixels` return
`HeifError::ImageTooLarge { width, height, max_pixels }`. HEVC decoder limit
violations may return `HeifError::Codec`; invalid grid aggregates and other
container resource violations return `HeifError::Invalid`. `HeifError` is
non-exhaustive, so downstream matches need a wildcard arm.

## What works

| Feature | Status |
| --- | --- |
| Single-picture `hvc1` items | ✅ |
| Grid-tiled images (every iPhone photo) | ✅ tiles decoded in parallel |
| 8-bit and 10-bit HEVC (Main / Main 10) | ✅ 10-bit → 16-bit output |
| Orientation (`irot` / `imir` / `clap`) | ✅ applied in `ipma` order |
| BT.601 / BT.709 / BT.2020 matrices, full & limited range | ✅ |
| Colour signalling from `colr` nclx, else the HEVC bitstream (SPS VUI) | ✅ iPhone photos carry only the latter |
| Display P3 → sRGB conversion (iPhone default) | ✅ linear-light 3×3 |
| Alpha auxiliary images | ⚠️ only when 4:2:0-coded (monochrome alpha needs a 4:0:0 decoder) |
| `idat`-stored payloads, multi-extent items | ✅ |
| `wasm32` targets | ✅ builds; decodes on a single thread there |
| Malformed input | errors, never panics (every parse is bounds-checked) |

Output is always display-ready sRGB.

## What doesn't (yet)

- Encoding — there is no pure-Rust HEVC encoder to build on.
- AVIF (`av01`) and JPEG-in-HEIF payloads — rejected with a clear error
  naming the codec.
- Image sequences (track/`moov`-based files), identity-derived (`iden`)
  items, overlays (`iovl`), protected items, external data references.
- Multi-layer (`lhv1`) items. When the primary item is the `hvc1` base
  layer of a layered file, that base layer decodes.
- 4:4:4 streams are rejected with a clear error: `rust_h265` decodes them,
  but the colour pipeline here is 4:2:0-only.
- ICC profiles are reported (`ColorInfo::icc_present`) but not applied;
  PQ/HLG HDR transfers are not tone-mapped.

## Status

On the [Nokia HEIF conformance suite](https://github.com/nokiatech/heif_conformance),
46 of 63 files decode; the rest are rejected cleanly for the unsupported
features listed above. Tested against real iPhone photos.

The HEVC decoder is pure Rust. Its own benchmarks put `rust_h265` 0.4 at
roughly 1.5× slower than single-threaded FFmpeg on 8-bit video, and faster
on 10-bit. A 12-megapixel iPhone photo takes on the order of a second, with
grid tiles spread across cores.

## Testing

`cargo test` exercises the container parser against hand-built boxes,
the colour pipeline against hand-computed reference values, and the full
decode path against tiny losslessly-encoded x265 fixtures (committed under
`testdata/`, generated from flat frames with exactly known plane values —
see `src/lib_tests.rs`).

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

MIT OR Apache-2.0, at your option.
