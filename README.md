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

## What works

| Feature | Status |
| --- | --- |
| Single-picture `hvc1` items | ✅ |
| Grid-tiled images (every iPhone photo) | ✅ tiles decoded in parallel |
| 8-bit and 10-bit HEVC (Main / Main 10) | ✅ 10-bit → 16-bit output |
| Orientation (`irot` / `imir` / `clap`) | ✅ applied in `ipma` order |
| BT.601 / BT.709 / BT.2020 matrices, full & limited range | ✅ |
| Display P3 → sRGB conversion (iPhone default) | ✅ linear-light 3×3 |
| Alpha auxiliary images | ⚠️ only when 4:2:0-coded (monochrome alpha needs a 4:0:0 decoder) |
| `idat`-stored payloads, multi-extent items | ✅ |
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
on 10-bit. Moving from `rust_h265` 0.1 to 0.4 made decoding the
conformance files about 1.3× faster end to end. A 12-megapixel iPhone
photo takes on the order of a second, with grid tiles spread across cores.

## Testing

`cargo test` exercises the container parser against hand-built boxes,
the colour pipeline against hand-computed reference values, and the full
decode path against tiny losslessly-encoded x265 fixtures (committed under
`testdata/`, generated from flat frames with exactly known plane values —
see `src/lib_tests.rs`).

## License

MIT OR Apache-2.0, at your option.
