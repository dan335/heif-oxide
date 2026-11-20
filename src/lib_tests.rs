//! End-to-end decode tests: real x265-encoded codestreams (committed under
//! `testdata/`, generated losslessly from flat frames with exactly known
//! plane values) wrapped into HEIC containers by `test_builder`.

use super::*;
use crate::test_builder as tb;

/// Load a committed Annex B fixture and split it into (hvcC box, payload).
fn fixture_item(name: &str) -> (Vec<u8>, Vec<u8>) {
    let path = format!("{}/testdata/{name}.h265", env!("CARGO_MANIFEST_DIR"));
    let annex_b = std::fs::read(path).expect("read fixture");
    tb::annex_b_to_item(&annex_b)
}

/// BT.601 limited-range nclx — matches how the flat fixtures were authored.
fn nclx_601() -> Vec<u8> {
    tb::colr_nclx(1, 13, 6, false)
}

fn assert_flat_rgb8(image: &DecodedImage, expected: [u8; 3], tolerance: i16) {
    let Pixels::Rgb8(buf) = &image.pixels else {
        panic!("expected Rgb8 pixels");
    };
    assert_eq!(buf.len(), (image.width * image.height * 3) as usize);
    for px in buf.as_chunks::<3>().0 {
        for c in 0..3 {
            assert!(
                (px[c] as i16 - expected[c] as i16).abs() <= tolerance,
                "pixel {:?} != {:?}",
                &px[..3],
                expected
            );
        }
    }
}

/// Build a single-`hvc1` HEIC around a fixture codestream.
fn single_item_heic(fixture: &str) -> Vec<u8> {
    let (hvcc, payload) = fixture_item(fixture);
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, tb::ispe(64, 64)), (false, nclx_601())],
    }];
    tb::make_heic(&items, 1, &[])
}

/// Build a 1x2 grid HEIC (left/right fixture tiles) with `extra_grid_props`
/// appended to the grid item's property list.
fn grid_heic(
    left: &str,
    right: &str,
    out_w: u16,
    out_h: u16,
    extra_grid_props: Vec<(bool, Vec<u8>)>,
) -> Vec<u8> {
    let (hvcc_l, payload_l) = fixture_item(left);
    let (hvcc_r, payload_r) = fixture_item(right);
    let mut grid_props = vec![
        (false, tb::ispe(out_w as u32, out_h as u32)),
        (false, nclx_601()),
    ];
    grid_props.extend(extra_grid_props);
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 2, out_w, out_h),
            props: grid_props,
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload_l,
            props: vec![(true, hvcc_l), (false, tb::ispe(64, 64))],
        },
        tb::TestItem {
            id: 3,
            item_type: *b"hvc1",
            payload: payload_r,
            props: vec![(true, hvcc_r), (false, tb::ispe(64, 64))],
        },
    ];
    tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2, 3])])
}

/// Sample a pixel from an Rgb8 image.
fn px(image: &DecodedImage, x: u32, y: u32) -> [u8; 3] {
    let Pixels::Rgb8(buf) = &image.pixels else {
        panic!("expected Rgb8");
    };
    let i = ((y * image.width + x) * 3) as usize;
    [buf[i], buf[i + 1], buf[i + 2]]
}

fn is_reddish(p: [u8; 3]) -> bool {
    p[0] > 200 && p[1] < 50 && p[2] < 50
}

fn is_bluish(p: [u8; 3]) -> bool {
    p[2] > 200 && p[0] < 50 && p[1] < 50
}

/// A single flat-red picture decodes to the hand-computed sRGB value.
#[test]
fn decodes_single_hvc1_item() {
    let image = decode_bytes(&single_item_heic("flat_red_64")).expect("decode");
    assert_eq!((image.width, image.height), (64, 64));
    assert_eq!(image.channels(), 3);
    assert_flat_rgb8(&image, [254, 0, 0], 1);
    // The colr box was found and reported.
    let nclx = image.color.nclx.expect("nclx");
    assert_eq!(nclx.matrix, 6);
    assert!(!nclx.full_range);
}

/// Grid assembly: red tile left, blue tile right, cropped from the 128x64
/// canvas to a declared 100x50 output.
#[test]
fn decodes_grid_with_crop() {
    let file = grid_heic("flat_red_64", "flat_blue_64", 100, 50, vec![]);
    let image = decode_bytes(&file).expect("decode");
    assert_eq!((image.width, image.height), (100, 50));
    // Probe just inside each tile — the boundary column itself legitimately
    // blends chroma from both tiles (bilinear 4:2:0 upsampling).
    assert!(is_reddish(px(&image, 10, 25)), "{:?}", px(&image, 10, 25));
    assert!(is_reddish(px(&image, 60, 25)), "{:?}", px(&image, 60, 25));
    assert!(is_bluish(px(&image, 68, 25)), "{:?}", px(&image, 68, 25));
    assert!(is_bluish(px(&image, 99, 49)), "{:?}", px(&image, 99, 49));
}

/// irot on the grid item rotates the assembled image: after 90° CCW the
/// right (blue) half becomes the top half.
#[test]
fn applies_grid_rotation() {
    let file = grid_heic(
        "flat_red_64",
        "flat_blue_64",
        100,
        50,
        vec![(true, tb::irot(1))],
    );
    let image = decode_bytes(&file).expect("decode");
    assert_eq!((image.width, image.height), (50, 100));
    assert!(is_bluish(px(&image, 25, 5)), "{:?}", px(&image, 25, 5));
    assert!(is_reddish(px(&image, 25, 95)), "{:?}", px(&image, 25, 95));
}

/// imir axis 1 exchanges left and right of the assembled image; axis 0
/// exchanges top and bottom, which leaves a left|right split where it was.
/// (Checked against libheif, which decodes the same files this way.)
#[test]
fn applies_grid_mirror() {
    let file = grid_heic(
        "flat_red_64",
        "flat_blue_64",
        100,
        50,
        vec![(true, tb::imir(1))],
    );
    let image = decode_bytes(&file).expect("decode");
    assert!(is_bluish(px(&image, 5, 25)), "{:?}", px(&image, 5, 25));
    assert!(is_reddish(px(&image, 95, 25)), "{:?}", px(&image, 95, 25));

    let file = grid_heic(
        "flat_red_64",
        "flat_blue_64",
        100,
        50,
        vec![(true, tb::imir(0))],
    );
    let image = decode_bytes(&file).expect("decode");
    assert!(is_reddish(px(&image, 5, 25)), "{:?}", px(&image, 5, 25));
    assert!(is_bluish(px(&image, 95, 25)), "{:?}", px(&image, 95, 25));
}

/// A 10-bit source comes out as full-scale Rgb16.
#[test]
fn decodes_ten_bit_to_rgb16() {
    let (hvcc, payload) = fixture_item("flat_mid_64_10bit");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, nclx_601())],
    }];
    let image = decode_bytes(&tb::make_heic(&items, 1, &[])).expect("decode");
    assert_eq!(image.bit_depth(), 16);
    let Pixels::Rgb16(buf) = &image.pixels else {
        panic!("expected Rgb16");
    };
    // Limited-range 10-bit Y=512 → (512-64)/876 → 33516 at u16 scale.
    for &s in buf {
        assert!((s as i32 - 33516).abs() <= 2, "sample {s}");
    }
}

/// A 4:2:0-coded alpha auxiliary is decoded and interleaved.
#[test]
fn decodes_alpha_auxiliary() {
    let (hvcc_main, payload_main) = fixture_item("flat_red_64");
    let (hvcc_alpha, payload_alpha) = fixture_item("flat_gray_64");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"hvc1",
            payload: payload_main,
            props: vec![(true, hvcc_main), (false, nclx_601())],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload_alpha,
            props: vec![
                (true, hvcc_alpha),
                (
                    false,
                    tb::full_box(b"auxC", 0, 0, b"urn:mpeg:hevc:2015:auxid:1\0"),
                ),
            ],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"auxl", 2, vec![1])]);
    let image = decode_bytes(&file).expect("decode");
    assert_eq!(image.channels(), 4);
    let Pixels::Rgba8(buf) = &image.pixels else {
        panic!("expected Rgba8");
    };
    // Alpha = luma 128 read full-range → 128; colour channels still red.
    for p in buf.as_chunks::<4>().0 {
        assert!((p[0] as i16 - 254).abs() <= 1 && p[1] <= 1 && p[2] <= 1);
        assert!((p[3] as i16 - 128).abs() <= 1, "alpha {}", p[3]);
    }
}

#[test]
fn rejects_amplifying_grids_before_decoding() {
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 2, 128, 64),
            props: vec![],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: vec![0; 16],
            props: vec![],
        },
    ];
    let duplicate_file = tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2, 2])]);
    assert!(matches!(
        decode_bytes(&duplicate_file),
        Err(HeifError::Invalid(message)) if message.contains("duplicate tile")
    ));

    let items = [tb::TestItem {
        id: 1,
        item_type: *b"grid",
        payload: tb::grid_payload(33, 33, 1, 1),
        props: vec![],
    }];
    assert!(matches!(
        decode_bytes(&tb::make_heic(&items, 1, &[])),
        Err(HeifError::Invalid(message)) if message.contains("maximum is 1024")
    ));

    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 2, 1, 1),
            props: vec![],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: vec![0; 16],
            props: vec![(false, tb::ispe(10_000, 10_000))],
        },
        tb::TestItem {
            id: 3,
            item_type: *b"hvc1",
            payload: vec![0; 16],
            props: vec![(false, tb::ispe(10_000, 10_000))],
        },
    ];
    let oversized_tiles = tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2, 3])]);
    assert!(matches!(
        decode_bytes(&oversized_tiles),
        Err(HeifError::Invalid(message)) if message.contains("tile images exceed")
    ));
}

#[test]
fn rejects_excessive_transforms_before_decoding() {
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload: vec![0; 16],
        props: (0..5).map(|_| (false, tb::irot(1))).collect(),
    }];
    assert!(matches!(
        decode_bytes(&tb::make_heic(&items, 1, &[])),
        Err(HeifError::Invalid(message)) if message.contains("5 transforms")
    ));
}

#[test]
fn rejects_oversized_image_before_decoding() {
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload: vec![0; 16],
        props: vec![(false, tb::ispe(65, 64))],
    }];
    let limits = Limits {
        max_pixels: 64 * 64,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&tb::make_heic(&items, 1, &[]), &limits),
        Err(HeifError::ImageTooLarge {
            width: 65,
            height: 64,
            max_pixels: 4096,
        })
    ));
}

#[test]
fn default_limits() {
    let limits = Limits::default();
    assert_eq!(limits.max_pixels, 100_000_000);
    assert_eq!(limits.max_grid_tiles, 1024);
    assert_eq!(limits.max_item_payload_bytes, 256 * 1024 * 1024);
    assert_eq!(limits.max_picture_memory_bytes, 512 * 1024 * 1024);
}

/// `max_pixels` lowers the per-picture bound but never raises it past the
/// largest picture an HEVC level allows — the default (sized for whole grid
/// canvases) must not loosen what a single picture may allocate.
#[test]
fn per_picture_pixel_bound_is_capped() {
    const HEVC_MAX_LUMA_PICTURE_SIZE: u64 = 35_651_584;
    assert_eq!(
        hevc::max_picture_pixels(&Limits::default()),
        HEVC_MAX_LUMA_PICTURE_SIZE
    );
    let huge = Limits {
        max_pixels: usize::MAX,
        ..Limits::default()
    };
    assert_eq!(hevc::max_picture_pixels(&huge), HEVC_MAX_LUMA_PICTURE_SIZE);
    let small = Limits {
        max_pixels: 4096,
        ..Limits::default()
    };
    assert_eq!(hevc::max_picture_pixels(&small), 4096);
}

#[test]
fn decodes_image_at_pixel_limit() {
    let limits = Limits {
        max_pixels: 64 * 64,
        ..Limits::default()
    };
    let image = decode_bytes_with(&single_item_heic("flat_red_64"), &limits).expect("decode");
    assert_eq!((image.width, image.height), (64, 64));
    assert_flat_rgb8(&image, [254, 0, 0], 1);
}

#[test]
fn rejects_spoofed_dimensions_using_bitstream_pixel_limit() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, tb::ispe(8, 8))],
    }];
    let file = tb::make_heic(&items, 1, &[]);
    assert_eq!(decode_bytes(&file).expect("decode").width, 64);
    let limits = Limits {
        max_pixels: 256,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Codec(_))
            | Err(HeifError::ImageTooLarge {
                width: 64,
                height: 64,
                max_pixels: 256,
            })
    ));
}

#[test]
fn enforces_bitstream_pixel_limit_without_ispe() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, nclx_601())],
    }];
    let file = tb::make_heic(&items, 1, &[]);
    let limits = Limits {
        max_pixels: 256,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Codec(_))
    ));
    let image = decode_bytes(&file).expect("decode without ispe");
    assert_eq!((image.width, image.height), (64, 64));
    assert_flat_rgb8(&image, [254, 0, 0], 1);
}

#[test]
fn enforces_bitstream_pixel_limit_on_grid_tiles() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 1, 8, 8),
            props: vec![],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload,
            props: vec![(true, hvcc), (false, tb::ispe(8, 8))],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2])]);
    let image = decode_bytes(&file).expect("decode");
    assert_eq!((image.width, image.height), (8, 8));
    let limits = Limits {
        max_pixels: 256,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Codec(_))
            | Err(HeifError::ImageTooLarge {
                width: 64,
                height: 64,
                max_pixels: 256,
            })
    ));
}

#[test]
fn enforces_actual_grid_aggregate_before_cropping() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 2, 8, 8),
            props: vec![(false, tb::ispe(8, 8)), (false, nclx_601())],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload.clone(),
            props: vec![(true, hvcc.clone()), (false, tb::ispe(8, 8))],
        },
        tb::TestItem {
            id: 3,
            item_type: *b"hvc1",
            payload,
            props: vec![(true, hvcc), (false, tb::ispe(8, 8))],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2, 3])]);
    let mut limits = Limits {
        max_pixels: 4096,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Codec(_))
    ));
    limits.max_pixels = 8192;
    let image = decode_bytes_with(&file, &limits).expect("decode at actual grid aggregate limit");
    assert_eq!((image.width, image.height), (8, 8));
    assert_flat_rgb8(&image, [254, 0, 0], 1);
}

#[test]
fn propagates_pixel_limit_to_alpha_decoder() {
    let (hvcc_main, payload_main) = fixture_item("flat_red_64");
    let (hvcc_alpha, payload_alpha) = fixture_item("flat_gray_64");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"hvc1",
            payload: payload_main,
            props: vec![(true, hvcc_main), (false, nclx_601())],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload_alpha,
            props: vec![
                (true, hvcc_alpha),
                (
                    false,
                    tb::full_box(b"auxC", 0, 0, b"urn:mpeg:hevc:2015:auxid:1\0"),
                ),
            ],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"auxl", 2, vec![1])]);
    let container = boxes::parse(&file).expect("parse");
    let limits = Limits {
        max_pixels: 256,
        ..Limits::default()
    };
    assert!(decode_alpha_plane(&container, 1, 64, 64, &limits).is_none());
    let alpha = decode_alpha_plane(&container, 1, 64, 64, &Limits::default())
        .expect("decode alpha with default limits");
    assert_eq!(alpha.len(), 64 * 64);
    for sample in alpha {
        assert!((i32::from(sample) - 128 * 257).abs() <= 257);
    }
}

#[test]
fn rejects_grid_output_above_pixel_limit_before_decoding() {
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"grid",
        payload: tb::grid_payload(1, 1, 65, 64),
        props: vec![(false, tb::ispe(8, 8))],
    }];
    let limits = Limits {
        max_pixels: 64 * 64,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&tb::make_heic(&items, 1, &[]), &limits),
        Err(HeifError::ImageTooLarge {
            width: 65,
            height: 64,
            max_pixels: 4096,
        })
    ));
}

#[test]
fn enforces_custom_picture_memory_limit() {
    let file = single_item_heic("flat_red_64");
    let mut limits = Limits {
        max_picture_memory_bytes: 1,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Codec(_))
    ));
    limits.max_picture_memory_bytes = Limits::default().max_picture_memory_bytes;
    decode_bytes_with(&file, &limits).expect("decode with default picture memory limit");
}

#[test]
fn enforces_custom_grid_tile_limit() {
    let file = grid_heic("flat_red_64", "flat_blue_64", 100, 50, vec![]);
    let mut limits = Limits {
        max_grid_tiles: 1,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Invalid(message)) if message.contains("maximum is 1")
    ));
    limits.max_grid_tiles = 2;
    let image = decode_bytes_with(&file, &limits).expect("decode at tile limit");
    assert_eq!((image.width, image.height), (100, 50));
}

#[test]
fn enforces_custom_item_payload_limit() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let payload_len = payload.len();
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, tb::ispe(64, 64)), (false, nclx_601())],
    }];
    let file = tb::make_heic(&items, 1, &[]);
    let mut limits = Limits {
        max_item_payload_bytes: payload_len - 1,
        ..Limits::default()
    };
    assert!(matches!(
        decode_bytes_with(&file, &limits),
        Err(HeifError::Invalid(_))
    ));
    limits.max_item_payload_bytes = payload_len;
    let image = decode_bytes_with(&file, &limits).expect("decode at payload limit");
    assert_flat_rgb8(&image, [254, 0, 0], 1);
}

#[test]
fn reads_metadata_without_decoding_pixels() {
    let (hvcc, _) = fixture_item("flat_red_64");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload: vec![0xFF],
        props: vec![(true, hvcc), (false, tb::ispe(64, 48)), (false, nclx_601())],
    }];
    let metadata = read_metadata(&tb::make_heic(&items, 1, &[])).expect("metadata");
    assert_eq!((metadata.width, metadata.height), (64, 48));
    assert_eq!(metadata.bit_depth, 8);
    assert!(!metadata.has_alpha);
    assert_eq!(metadata.color.nclx.expect("nclx").matrix, 6);
}

#[test]
fn reads_grid_metadata_with_orientation() {
    let file = grid_heic(
        "flat_red_64",
        "flat_blue_64",
        100,
        50,
        vec![(true, tb::irot(1))],
    );
    let metadata = read_metadata(&file).expect("metadata");
    assert_eq!((metadata.width, metadata.height), (50, 100));
    assert_eq!(metadata.bit_depth, 8);
}

#[test]
fn reads_ten_bit_and_alpha_metadata() {
    let (mut hvcc_main, payload_main) = fixture_item("flat_mid_64_10bit");
    hvcc_main[25] = 0xFA;
    let (hvcc_alpha, payload_alpha) = fixture_item("flat_gray_64");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"hvc1",
            payload: payload_main,
            props: vec![(true, hvcc_main), (false, tb::ispe(64, 64))],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload_alpha,
            props: vec![
                (true, hvcc_alpha),
                (
                    false,
                    tb::full_box(b"auxC", 0, 0, b"urn:mpeg:hevc:2015:auxid:1\0"),
                ),
            ],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"auxl", 2, vec![1])]);
    let metadata = read_metadata(&file).expect("metadata");
    assert_eq!(metadata.bit_depth, 10);
    assert!(metadata.has_alpha);
}

/// Non-HEVC primary items are rejected with the codec named.
#[test]
fn rejects_av01_item() {
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"av01",
        payload: vec![0; 16],
        props: vec![],
    }];
    match decode_bytes(&tb::make_heic(&items, 1, &[])) {
        Err(HeifError::UnsupportedCodec(codec)) => assert_eq!(codec, "av01"),
        other => panic!("expected UnsupportedCodec, got {:?}", other.err()),
    }
}

/// Unknown *essential* properties abort the decode (spec-required).
#[test]
fn rejects_unknown_essential_property() {
    let (hvcc, payload) = fixture_item("flat_red_64");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (true, tb::plain_box(b"zzzz", &[1, 2, 3]))],
    }];
    match decode_bytes(&tb::make_heic(&items, 1, &[])) {
        Err(HeifError::Unsupported(msg)) => assert!(msg.contains("zzzz"), "{msg}"),
        other => panic!("expected Unsupported, got {:?}", other.err()),
    }
}

/// Corrupt HEVC payload bytes surface as a codec error, not a panic.
#[test]
fn corrupt_payload_is_an_error() {
    let (hvcc, mut payload) = fixture_item("flat_red_64");
    // Scramble the slice data (keep the first length prefix plausible).
    for b in payload.iter_mut().skip(8) {
        *b ^= 0xA5;
    }
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc)],
    }];
    assert!(decode_bytes(&tb::make_heic(&items, 1, &[])).is_err());
}

/// A 4:4:4 codestream decodes in rust_h265 but is rejected cleanly here
/// (the colour pipeline is 4:2:0-only).
#[test]
fn rejects_444_chroma() {
    match decode_bytes(&single_item_heic("flat_red_64_444")) {
        Err(HeifError::Unsupported(msg)) => assert!(msg.contains("chroma_format_idc 3"), "{msg}"),
        other => panic!("expected Unsupported, got {:?}", other.err()),
    }
}

/// `flat_full_range_64` is a flat Y=60 Cb=110 Cr=160 picture whose SPS VUI
/// signals what an iPhone writes: full range, BT.601 matrix, Display P3
/// primaries. Decoded with that description it is this sRGB colour; with
/// the limited-range BT.709 defaults it would be [109, 38, 13].
const FULL_RANGE_P3_RGB: [u8; 3] = [114, 38, 22];

/// A file with no `colr` nclx box takes range, matrix and primaries from
/// the bitstream's VUI instead of assuming limited-range BT.709.
#[test]
fn bitstream_colour_is_used_without_nclx() {
    let (hvcc, payload) = fixture_item("flat_full_range_64");
    let items = [tb::TestItem {
        id: 1,
        item_type: *b"hvc1",
        payload,
        props: vec![(true, hvcc), (false, tb::ispe(64, 64))],
    }];
    let image = decode_bytes(&tb::make_heic(&items, 1, &[])).expect("decode");
    assert_flat_rgb8(&image, FULL_RANGE_P3_RGB, 1);
    // `color.nclx` reports the container, which declared nothing.
    assert!(image.color.nclx.is_none());
}

/// The iPhone layout: a grid whose only `colr` is an ICC profile. The
/// tiles' VUI still drives the conversion.
#[test]
fn bitstream_colour_is_used_for_grid_with_icc_only() {
    let (hvcc_l, payload_l) = fixture_item("flat_full_range_64");
    let (hvcc_r, payload_r) = fixture_item("flat_full_range_64");
    let icc = tb::plain_box(b"colr", b"prof\0\0\0\0");
    let items = [
        tb::TestItem {
            id: 1,
            item_type: *b"grid",
            payload: tb::grid_payload(1, 2, 100, 50),
            props: vec![(false, tb::ispe(100, 50)), (false, icc)],
        },
        tb::TestItem {
            id: 2,
            item_type: *b"hvc1",
            payload: payload_l,
            props: vec![(true, hvcc_l), (false, tb::ispe(64, 64))],
        },
        tb::TestItem {
            id: 3,
            item_type: *b"hvc1",
            payload: payload_r,
            props: vec![(true, hvcc_r), (false, tb::ispe(64, 64))],
        },
    ];
    let file = tb::make_heic(&items, 1, &[(*b"dimg", 1, vec![2, 3])]);
    let image = decode_bytes(&file).expect("decode");
    assert_eq!((image.width, image.height), (100, 50));
    assert_flat_rgb8(&image, FULL_RANGE_P3_RGB, 1);
    assert!(image.color.icc_present);
    assert!(image.color.nclx.is_none());
}

/// A container nclx box, when present, wins over the bitstream's VUI.
#[test]
fn container_nclx_overrides_bitstream_colour() {
    // single_item_heic attaches a limited-range BT.601 sRGB nclx.
    let image = decode_bytes(&single_item_heic("flat_full_range_64")).expect("decode");
    assert_flat_rgb8(&image, [102, 32, 15], 1);
    assert!(image.color.nclx.is_some());
}

/// Not a test: writes a small complete `.heic` (the flat-red codestream in
/// a single-item container) to `testdata/` for downstream consumers that
/// want a committed end-to-end fixture. Run explicitly with
/// `cargo test generate_sample_heic -- --ignored`.
#[test]
#[ignore = "fixture generator, writes into testdata/"]
fn generate_sample_heic() {
    let file = single_item_heic("flat_red_64");
    decode_bytes(&file).expect("generated fixture must decode");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/flat_red_64.heic");
    std::fs::write(path, file).expect("write fixture");
}

/// `to_f32_interleaved` and `to_rgba8` round sensibly.
#[test]
fn conversion_helpers() {
    let image = decode_bytes(&single_item_heic("flat_gray_64")).expect("decode");
    let f = image.to_f32_interleaved();
    assert_eq!(f.len(), 64 * 64 * 3);
    assert!((f[0] - 130.0 / 255.0).abs() < 0.01);
    let rgba = image.to_rgba8();
    assert_eq!(rgba.len(), 64 * 64 * 4);
    assert_eq!(rgba[3], 255);
}
