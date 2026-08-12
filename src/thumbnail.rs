//! Target-sized HEIF decode path used by thumbnail generators.
//!
//! The regular decoder intentionally returns every source pixel. This module
//! instead selects a suitable embedded `thmb` item when available and writes
//! grid tiles into a target-sized RGB canvas one at a time. It never builds a
//! full-image grid canvas.

use crate::boxes::{Container, Property};
use crate::color::{yuv_to_rgb_scaled, RgbBuffer};
use crate::{
    collect_color_info, decode_bytes, decode_coded_item, ColorInfo, DecodedImage, HeifError, Pixels,
};

const CODED_PIXEL_PEAK_BYTES: u64 = 16;
const OUTPUT_PIXEL_PEAK_BYTES: u64 = 12;
const FULL_DECODE_PIXEL_PEAK_BYTES: u64 = 20;

/// The selected decode path and its conservative peak-memory estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailDecodeInfo {
    pub selected_item_id: u32,
    pub used_embedded_thumbnail: bool,
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub estimated_peak_bytes: u64,
    pub grid_streaming: bool,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone)]
struct DecodePlan {
    selected_item_id: u32,
    used_embedded_thumbnail: bool,
    source_width: usize,
    source_height: usize,
    crop: Rect,
    output_width: usize,
    output_height: usize,
    pre_transform_width: usize,
    pre_transform_height: usize,
    transforms: Vec<Property>,
    optimized: bool,
    grid_streaming: bool,
}

struct Grid {
    rows: usize,
    columns: usize,
    output_width: usize,
    output_height: usize,
}

/// Inspect the item that a target-sized decode would use and estimate peak
/// memory without decoding pixels.
pub fn thumbnail_info(
    data: &[u8],
    max_width: u32,
    max_height: u32,
) -> Result<ThumbnailDecodeInfo, HeifError> {
    validate_target(max_width, max_height)?;
    let container = crate::boxes::parse(data)?;
    let plan = build_plan(&container, max_width as usize, max_height as usize)?;
    let estimated_peak_bytes = estimate_peak_bytes(&container, &plan, data.len())?;
    Ok(ThumbnailDecodeInfo {
        selected_item_id: plan.selected_item_id,
        used_embedded_thumbnail: plan.used_embedded_thumbnail,
        source_width: plan.source_width as u32,
        source_height: plan.source_height as u32,
        output_width: plan.output_width as u32,
        output_height: plan.output_height as u32,
        estimated_peak_bytes,
        grid_streaming: plan.grid_streaming,
    })
}

/// Decode directly to the largest aspect-preserving image inside the target
/// bounds. Embedded thumbnails are preferred when they contain enough source
/// pixels. Grid tiles are decoded sequentially into the scaled output.
pub fn decode_thumbnail_bytes(
    data: &[u8],
    max_width: u32,
    max_height: u32,
) -> Result<DecodedImage, HeifError> {
    validate_target(max_width, max_height)?;
    let container = crate::boxes::parse(data)?;
    let plan = build_plan(&container, max_width as usize, max_height as usize)?;
    if !plan.optimized {
        return resize_decoded(decode_bytes(data)?, max_width as usize, max_height as usize);
    }

    let color = collect_color_info(&container, plan.selected_item_id);
    let nclx = color.nclx.unwrap_or_default();
    let item = container
        .items
        .get(&plan.selected_item_id)
        .ok_or_else(|| HeifError::Invalid("selected thumbnail item is missing".into()))?;
    let rgb = match &item.item_type {
        b"hvc1" => decode_coded_scaled(&container, &plan, &nclx)?,
        b"grid" => decode_grid_scaled(&container, &plan, &nclx)?,
        other => {
            return Err(HeifError::UnsupportedCodec(crate::bytes::fourcc_str(
                *other,
            )))
        }
    };
    finish_scaled(rgb, color, &plan)
}

fn validate_target(max_width: u32, max_height: u32) -> Result<(), HeifError> {
    if max_width == 0 || max_height == 0 {
        return Err(HeifError::Invalid(
            "thumbnail target dimensions must be non-zero".into(),
        ));
    }
    Ok(())
}

fn build_plan(
    container: &Container<'_>,
    max_width: usize,
    max_height: usize,
) -> Result<DecodePlan, HeifError> {
    let primary = container.primary_item;
    let primary_dimensions = item_dimensions(container, primary)?;
    let primary_transforms = item_transforms(container, primary);
    let primary_geometry = geometry(primary_dimensions, &primary_transforms)?;
    let desired = fit_dimensions(
        primary_geometry.1,
        primary_geometry.2,
        max_width,
        max_height,
    );
    let required_source = (
        desired.0.min(primary_geometry.1),
        desired.1.min(primary_geometry.2),
    );

    let mut candidates = container
        .references
        .iter()
        .filter(|reference| &reference.ref_type == b"thmb" && reference.to_items.contains(&primary))
        .filter_map(|reference| {
            let item_id = reference.from_item;
            let dimensions = item_dimensions(container, item_id).ok()?;
            let own_transforms = item_transforms(container, item_id);
            let transforms = if own_transforms.is_empty() {
                primary_transforms
                    .iter()
                    .filter(|property| {
                        matches!(property, Property::Irot { .. } | Property::Imir { .. })
                    })
                    .cloned()
                    .collect()
            } else {
                own_transforms
            };
            let item_geometry = geometry(dimensions, &transforms).ok()?;
            if item_geometry.1 < required_source.0
                || item_geometry.2 < required_source.1
                || !similar_aspect(
                    item_geometry.1,
                    item_geometry.2,
                    primary_geometry.1,
                    primary_geometry.2,
                )
            {
                return None;
            }
            Some((
                item_geometry.1.saturating_mul(item_geometry.2),
                item_id,
                dimensions,
                transforms,
                item_geometry,
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.0);

    let (selected_item_id, dimensions, transforms, selected_geometry, embedded) = candidates
        .into_iter()
        .next()
        .map(|(_, item, dimensions, transforms, geometry)| {
            (item, dimensions, transforms, geometry, true)
        })
        .unwrap_or((
            primary,
            primary_dimensions,
            primary_transforms,
            primary_geometry,
            false,
        ));
    let selected_output = fit_dimensions(
        selected_geometry.1,
        selected_geometry.2,
        max_width,
        max_height,
    );
    let quarter_turns = net_quarter_turns(&transforms);
    let (pre_transform_width, pre_transform_height) = if quarter_turns % 2 == 1 {
        (selected_output.1, selected_output.0)
    } else {
        selected_output
    };
    let selected_item = &container.items[&selected_item_id];
    let has_alpha = alpha_item(container, selected_item_id).is_some();
    let optimized = selected_geometry.3 && !has_alpha;

    Ok(DecodePlan {
        selected_item_id,
        used_embedded_thumbnail: embedded,
        source_width: dimensions.0,
        source_height: dimensions.1,
        crop: selected_geometry.0,
        output_width: selected_output.0,
        output_height: selected_output.1,
        pre_transform_width,
        pre_transform_height,
        transforms,
        optimized,
        grid_streaming: optimized && selected_item.item_type == *b"grid",
    })
}

/// Returns `(crop, displayed width, displayed height, optimized transform order)`.
fn geometry(
    dimensions: (usize, usize),
    transforms: &[Property],
) -> Result<(Rect, usize, usize, bool), HeifError> {
    let mut crop = Rect {
        x: 0,
        y: 0,
        width: dimensions.0,
        height: dimensions.1,
    };
    let mut width = dimensions.0;
    let mut height = dimensions.1;
    let mut saw_orientation = false;
    let mut optimized = true;
    for transform in transforms {
        match transform {
            Property::Clap {
                width_n,
                width_d,
                height_n,
                height_d,
                horiz_off_n,
                horiz_off_d,
                vert_off_n,
                vert_off_d,
            } => {
                if saw_orientation || crop.x != 0 || crop.y != 0 {
                    optimized = false;
                    continue;
                }
                crop = clean_aperture_rect(
                    dimensions,
                    *width_n,
                    *width_d,
                    *height_n,
                    *height_d,
                    *horiz_off_n,
                    *horiz_off_d,
                    *vert_off_n,
                    *vert_off_d,
                )?;
                width = crop.width;
                height = crop.height;
            }
            Property::Irot { angle } => {
                saw_orientation = true;
                if angle % 2 == 1 {
                    (width, height) = (height, width);
                }
            }
            Property::Imir { .. } => saw_orientation = true,
            _ => {}
        }
    }
    Ok((crop, width, height, optimized))
}

#[allow(clippy::too_many_arguments)]
fn clean_aperture_rect(
    dimensions: (usize, usize),
    width_n: u32,
    width_d: u32,
    height_n: u32,
    height_d: u32,
    horizontal_offset_n: i32,
    horizontal_offset_d: u32,
    vertical_offset_n: i32,
    vertical_offset_d: u32,
) -> Result<Rect, HeifError> {
    let fraction = |numerator: i64, denominator: u32| {
        if denominator == 0 {
            0.0
        } else {
            numerator as f64 / denominator as f64
        }
    };
    let width = fraction(width_n as i64, width_d).round() as i64;
    let height = fraction(height_n as i64, height_d).round() as i64;
    if width <= 0 || height <= 0 || width as usize > dimensions.0 || height as usize > dimensions.1
    {
        return Err(HeifError::Invalid(
            "clean aperture is outside the image".into(),
        ));
    }
    let center_x = (dimensions.0 as f64 - 1.0) / 2.0
        + fraction(horizontal_offset_n as i64, horizontal_offset_d);
    let center_y =
        (dimensions.1 as f64 - 1.0) / 2.0 + fraction(vertical_offset_n as i64, vertical_offset_d);
    let x = (center_x - (width as f64 - 1.0) / 2.0).round() as i64;
    let y = (center_y - (height as f64 - 1.0) / 2.0).round() as i64;
    if x < 0 || y < 0 || x + width > dimensions.0 as i64 || y + height > dimensions.1 as i64 {
        return Err(HeifError::Invalid(
            "clean aperture extends outside the image".into(),
        ));
    }
    Ok(Rect {
        x: x as usize,
        y: y as usize,
        width: width as usize,
        height: height as usize,
    })
}

fn fit_dimensions(
    width: usize,
    height: usize,
    max_width: usize,
    max_height: usize,
) -> (usize, usize) {
    let width_limited_height = height.saturating_mul(max_width) / width;
    if width_limited_height <= max_height {
        (max_width, width_limited_height.max(1))
    } else {
        (
            (width.saturating_mul(max_height) / height).max(1),
            max_height,
        )
    }
}

fn similar_aspect(a_width: usize, a_height: usize, b_width: usize, b_height: usize) -> bool {
    let lhs = (a_width as u128) * (b_height as u128);
    let rhs = (b_width as u128) * (a_height as u128);
    let difference = lhs.abs_diff(rhs);
    difference.saturating_mul(100) <= lhs.max(rhs)
}

fn item_dimensions(container: &Container<'_>, item_id: u32) -> Result<(usize, usize), HeifError> {
    let item = container
        .items
        .get(&item_id)
        .ok_or_else(|| HeifError::Invalid(format!("item {item_id} is missing")))?;
    if item.item_type == *b"grid" {
        let grid = parse_grid(container, item_id)?;
        return Ok((grid.output_width, grid.output_height));
    }
    container
        .find_property(item_id, |property| match property {
            Property::Ispe { width, height } if *width != 0 && *height != 0 => {
                Some((*width as usize, *height as usize))
            }
            _ => None,
        })
        .ok_or_else(|| HeifError::Invalid(format!("image item {item_id} has no dimensions")))
}

fn item_transforms(container: &Container<'_>, item_id: u32) -> Vec<Property> {
    container
        .item_properties(item_id)
        .filter_map(|(_, property)| match property {
            Property::Clap { .. } | Property::Irot { .. } | Property::Imir { .. } => {
                Some(property.clone())
            }
            _ => None,
        })
        .collect()
}

fn net_quarter_turns(transforms: &[Property]) -> u8 {
    transforms
        .iter()
        .fold(0, |turns, transform| match transform {
            Property::Irot { angle } => (turns + angle) % 4,
            _ => turns,
        })
}

fn alpha_item(container: &Container<'_>, primary: u32) -> Option<u32> {
    container.references.iter().find_map(|reference| {
        if &reference.ref_type != b"auxl" || !reference.to_items.contains(&primary) {
            return None;
        }
        container
            .find_property(reference.from_item, |property| match property {
                Property::AuxC(urn)
                    if urn.contains("alpha") || urn == "urn:mpeg:hevc:2015:auxid:1" =>
                {
                    Some(())
                }
                _ => None,
            })
            .map(|()| reference.from_item)
    })
}

fn parse_grid(container: &Container<'_>, item_id: u32) -> Result<Grid, HeifError> {
    let payload = container.item_payload(item_id)?;
    let mut reader = crate::bytes::Reader::new(&payload, "grid item payload");
    let version = reader.u8()?;
    if version != 0 {
        return Err(HeifError::Unsupported(format!(
            "grid item version {version}"
        )));
    }
    let flags = reader.u8()?;
    let rows = reader.u8()? as usize + 1;
    let columns = reader.u8()? as usize + 1;
    let (output_width, output_height) = if flags & 1 != 0 {
        (reader.u32()? as usize, reader.u32()? as usize)
    } else {
        (reader.u16()? as usize, reader.u16()? as usize)
    };
    if output_width == 0 || output_height == 0 {
        return Err(HeifError::Invalid("grid output size is zero".into()));
    }
    let tiles = container.referenced_items(item_id, b"dimg");
    if tiles.len() != rows.saturating_mul(columns) {
        return Err(HeifError::Invalid(format!(
            "grid declares {rows}x{columns} tiles but references {}",
            tiles.len()
        )));
    }
    Ok(Grid {
        rows,
        columns,
        output_width,
        output_height,
    })
}

fn decode_coded_scaled(
    container: &Container<'_>,
    plan: &DecodePlan,
    nclx: &crate::Nclx,
) -> Result<RgbBuffer, HeifError> {
    let yuv = decode_coded_item(container, plan.selected_item_id)?;
    if plan.crop.x + plan.crop.width > yuv.width || plan.crop.y + plan.crop.height > yuv.height {
        return Err(HeifError::Invalid(
            "declared image crop exceeds decoded HEVC dimensions".into(),
        ));
    }
    Ok(yuv_to_rgb_scaled(
        &yuv,
        nclx,
        plan.crop.x,
        plan.crop.y,
        plan.crop.width,
        plan.crop.height,
        plan.pre_transform_width,
        plan.pre_transform_height,
    ))
}

fn decode_grid_scaled(
    container: &Container<'_>,
    plan: &DecodePlan,
    nclx: &crate::Nclx,
) -> Result<RgbBuffer, HeifError> {
    let grid = parse_grid(container, plan.selected_item_id)?;
    let tiles = container.referenced_items(plan.selected_item_id, b"dimg");
    let first_dimensions = item_dimensions(container, tiles[0])?;
    if first_dimensions.0.saturating_mul(grid.columns) < grid.output_width
        || first_dimensions.1.saturating_mul(grid.rows) < grid.output_height
    {
        return Err(HeifError::Invalid(
            "grid canvas is smaller than its declared output".into(),
        ));
    }

    let mut output = None;
    for (index, tile_id) in tiles.into_iter().enumerate() {
        let tile_dimensions = item_dimensions(container, tile_id)?;
        if tile_dimensions != first_dimensions {
            return Err(HeifError::Invalid(
                "grid tiles have differing dimensions".into(),
            ));
        }
        let tile_x = (index % grid.columns) * first_dimensions.0;
        let tile_y = (index / grid.columns) * first_dimensions.1;
        let Some(intersection) = intersect(
            Rect {
                x: tile_x,
                y: tile_y,
                width: first_dimensions
                    .0
                    .min(grid.output_width.saturating_sub(tile_x)),
                height: first_dimensions
                    .1
                    .min(grid.output_height.saturating_sub(tile_y)),
            },
            plan.crop,
        ) else {
            continue;
        };
        let destination = destination_rect(intersection, plan.crop, plan);
        if destination.width == 0 || destination.height == 0 {
            continue;
        }

        let tile = decode_coded_item(container, tile_id)?;
        if (tile.width, tile.height) != first_dimensions {
            return Err(HeifError::Invalid(
                "grid tile dimensions differ from ispe".into(),
            ));
        }
        let scaled = yuv_to_rgb_scaled(
            &tile,
            nclx,
            intersection.x - tile_x,
            intersection.y - tile_y,
            intersection.width,
            intersection.height,
            destination.width,
            destination.height,
        );
        let output = output.get_or_insert_with(|| {
            RgbBuffer::black(
                plan.pre_transform_width,
                plan.pre_transform_height,
                tile.bit_depth,
            )
        });
        output.blit(
            &scaled,
            destination.width,
            destination.x,
            destination.y,
            plan.pre_transform_width,
        )?;
    }
    output.ok_or_else(|| HeifError::Invalid("grid crop selects no decodable tiles".into()))
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (x < right && y < bottom).then_some(Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

fn destination_rect(source: Rect, crop: Rect, plan: &DecodePlan) -> Rect {
    let left = (source.x - crop.x) * plan.pre_transform_width / crop.width;
    let top = (source.y - crop.y) * plan.pre_transform_height / crop.height;
    let right = (source.x + source.width - crop.x) * plan.pre_transform_width / crop.width;
    let bottom = (source.y + source.height - crop.y) * plan.pre_transform_height / crop.height;
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

fn finish_scaled(
    rgb: RgbBuffer,
    color: ColorInfo,
    plan: &DecodePlan,
) -> Result<DecodedImage, HeifError> {
    let transforms = plan
        .transforms
        .iter()
        .filter(|property| matches!(property, Property::Irot { .. } | Property::Imir { .. }))
        .cloned()
        .collect::<Vec<_>>();
    let (width, height, pixels) = match rgb {
        RgbBuffer::Rgb8(samples) => {
            let (samples, width, height) = crate::transform::apply_transforms(
                samples,
                plan.pre_transform_width,
                plan.pre_transform_height,
                3,
                &transforms,
            )?;
            (width, height, Pixels::Rgb8(samples))
        }
        RgbBuffer::Rgb16(samples) => {
            let (samples, width, height) = crate::transform::apply_transforms(
                samples,
                plan.pre_transform_width,
                plan.pre_transform_height,
                3,
                &transforms,
            )?;
            (width, height, Pixels::Rgb16(samples))
        }
    };
    if (width, height) != (plan.output_width, plan.output_height) {
        return Err(HeifError::Invalid(
            "scaled transform dimensions do not match the decode plan".into(),
        ));
    }
    Ok(DecodedImage {
        width: width as u32,
        height: height as u32,
        pixels,
        color,
    })
}

fn resize_decoded(
    image: DecodedImage,
    max_width: usize,
    max_height: usize,
) -> Result<DecodedImage, HeifError> {
    let dimensions = fit_dimensions(
        image.width as usize,
        image.height as usize,
        max_width,
        max_height,
    );
    if dimensions == (image.width as usize, image.height as usize) {
        return Ok(image);
    }
    let pixels = match image.pixels {
        Pixels::Rgb8(samples) => Pixels::Rgb8(resize_interleaved(
            &samples,
            image.width as usize,
            image.height as usize,
            dimensions,
            3,
        )),
        Pixels::Rgba8(samples) => Pixels::Rgba8(resize_interleaved(
            &samples,
            image.width as usize,
            image.height as usize,
            dimensions,
            4,
        )),
        Pixels::Rgb16(samples) => Pixels::Rgb16(resize_interleaved(
            &samples,
            image.width as usize,
            image.height as usize,
            dimensions,
            3,
        )),
        Pixels::Rgba16(samples) => Pixels::Rgba16(resize_interleaved(
            &samples,
            image.width as usize,
            image.height as usize,
            dimensions,
            4,
        )),
    };
    Ok(DecodedImage {
        width: dimensions.0 as u32,
        height: dimensions.1 as u32,
        pixels,
        color: image.color,
    })
}

fn resize_interleaved<T: Copy + Default>(
    source: &[T],
    source_width: usize,
    source_height: usize,
    destination: (usize, usize),
    channels: usize,
) -> Vec<T> {
    let mut output = vec![T::default(); destination.0 * destination.1 * channels];
    for y in 0..destination.1 {
        let source_y = ((2 * y + 1) * source_height / (2 * destination.1)).min(source_height - 1);
        for x in 0..destination.0 {
            let source_x = ((2 * x + 1) * source_width / (2 * destination.0)).min(source_width - 1);
            let source_index = (source_y * source_width + source_x) * channels;
            let destination_index = (y * destination.0 + x) * channels;
            output[destination_index..destination_index + channels]
                .copy_from_slice(&source[source_index..source_index + channels]);
        }
    }
    output
}

fn estimate_peak_bytes(
    container: &Container<'_>,
    plan: &DecodePlan,
    source_len: usize,
) -> Result<u64, HeifError> {
    let source_bytes = source_len as u64;
    if !plan.optimized {
        return Ok(source_bytes.saturating_add(
            (plan.source_width as u64)
                .saturating_mul(plan.source_height as u64)
                .saturating_mul(FULL_DECODE_PIXEL_PEAK_BYTES),
        ));
    }
    let item_ids = if plan.grid_streaming {
        container.referenced_items(plan.selected_item_id, b"dimg")
    } else {
        vec![plan.selected_item_id]
    };
    let mut max_pixels = 0_u64;
    let mut previous_coded_dimensions = None;
    for item_id in &item_ids {
        let declared = item_dimensions(container, *item_id)?;
        let coded = item_coded_dimensions(container, *item_id)?;
        if plan.grid_streaming
            && previous_coded_dimensions.is_some_and(|expected| expected != coded)
        {
            return Err(HeifError::Invalid(
                "grid tiles have differing coded dimensions".into(),
            ));
        }
        previous_coded_dimensions = Some(coded);
        max_pixels = max_pixels
            .max((declared.0.max(coded.0) as u64).saturating_mul(declared.1.max(coded.1) as u64));
    }
    let max_payload = item_ids
        .iter()
        .map(|item_id| item_payload_len(container, *item_id, source_len))
        .max()
        .unwrap_or(0);
    let output_pixels =
        (plan.pre_transform_width as u64).saturating_mul(plan.pre_transform_height as u64);
    Ok(source_bytes
        .saturating_add(max_pixels.saturating_mul(CODED_PIXEL_PEAK_BYTES))
        .saturating_add(output_pixels.saturating_mul(OUTPUT_PIXEL_PEAK_BYTES))
        .saturating_add(max_payload.saturating_mul(2)))
}

fn item_coded_dimensions(
    container: &Container<'_>,
    item_id: u32,
) -> Result<(usize, usize), HeifError> {
    let item = container
        .items
        .get(&item_id)
        .ok_or_else(|| HeifError::Invalid(format!("item {item_id} is missing")))?;
    if item.item_type != *b"hvc1" {
        return Err(HeifError::UnsupportedCodec(crate::bytes::fourcc_str(
            item.item_type,
        )));
    }
    let record = container
        .find_property(item_id, |property| match property {
            Property::HvcC(record) => Some(record.clone()),
            _ => None,
        })
        .ok_or_else(|| HeifError::Invalid(format!("coded item {item_id} has no hvcC property")))?;
    let (width, height) = crate::hevc::coded_dimensions(&record)?;
    Ok((width as usize, height as usize))
}

fn item_payload_len(container: &Container<'_>, item_id: u32, source_len: usize) -> u64 {
    container
        .locations
        .get(&item_id)
        .map(|location| {
            location
                .extents
                .iter()
                .fold(0_u64, |total, (offset, length)| {
                    total.saturating_add(if *length == 0 {
                        (source_len as u64)
                            .saturating_sub(location.base_offset.saturating_add(*offset))
                    } else {
                        *length
                    })
                })
        })
        .unwrap_or(0)
}
