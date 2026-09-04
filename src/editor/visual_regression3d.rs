//! Deterministic framebuffer comparison used by 3D Game View regression gates.

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

pub(crate) const BASELINE_METADATA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct VisualBaselineMetadata {
    pub version: u32,
    pub backend: String,
    pub width: u32,
    pub height: u32,
}

impl VisualBaselineMetadata {
    pub(crate) fn new(backend: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            version: BASELINE_METADATA_VERSION,
            backend: backend.into(),
            width,
            height,
        }
    }

    pub(crate) fn matches(&self, width: u32, height: u32) -> bool {
        self.version == BASELINE_METADATA_VERSION && self.width == width && self.height == height
    }
}

pub(crate) fn backend_family(backend: &str) -> &str {
    backend.strip_suffix("-embedded").unwrap_or(backend)
}

pub(crate) fn comparison_tolerance(
    baseline_backend: &str,
    current_backend: &str,
) -> (VisualTolerance, &'static str) {
    if !baseline_backend.is_empty()
        && backend_family(baseline_backend) != backend_family(current_backend)
    {
        (VisualTolerance::cross_backend(), "cross-backend-aa-aware")
    } else {
        (VisualTolerance::default(), "same-backend-strict")
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct VisualTolerance {
    /// A pixel is materially changed when any RGB channel exceeds this delta.
    pub channel_threshold: u8,
    /// Maximum fraction of materially changed pixels.
    pub changed_pixel_ratio: f32,
    /// Maximum mean absolute RGB-channel error across the complete image.
    pub mean_absolute_error: f32,
}

impl Default for VisualTolerance {
    fn default() -> Self {
        Self {
            channel_threshold: 8,
            changed_pixel_ratio: 0.01,
            mean_absolute_error: 1.5,
        }
    }
}

impl VisualTolerance {
    /// Native MSAA and the software edge filter can choose different coverage
    /// values on a one-pixel silhouette while agreeing on the rendered scene.
    /// Keep the per-channel and whole-image error limits strict, but permit the
    /// measured proportion of those edge pixels for an explicit cross-backend
    /// comparison.
    pub(crate) fn cross_backend() -> Self {
        Self {
            changed_pixel_ratio: 0.03,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PixelBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct VisualDiffReport {
    pub width: u32,
    pub height: u32,
    pub baseline_width: u32,
    pub baseline_height: u32,
    pub compared_pixels: u64,
    pub changed_pixels: u64,
    pub changed_pixel_ratio: f32,
    pub mean_absolute_error: f32,
    pub max_channel_error: u8,
    pub mismatch_bounds: Option<PixelBounds>,
    pub tolerance: VisualTolerance,
    #[serde(default)]
    pub comparison_profile: String,
    #[serde(default)]
    pub baseline_backend: String,
    #[serde(default)]
    pub current_backend: String,
    pub passed: bool,
    pub reason: String,
}

impl VisualDiffReport {
    pub(crate) fn summary(&self) -> String {
        if self.passed {
            format!(
                "PASS · {:.3}% changed · mean error {:.3} · max channel {}",
                self.changed_pixel_ratio * 100.0,
                self.mean_absolute_error,
                self.max_channel_error
            )
        } else {
            format!(
                "FAIL · {} · {:.3}% changed · mean error {:.3} · max channel {}",
                self.reason,
                self.changed_pixel_ratio * 100.0,
                self.mean_absolute_error,
                self.max_channel_error
            )
        }
    }
}

/// Compare two exact-sized RGBA framebuffers. Alpha participates in the diff
/// visualization but pass/fail metrics use display RGB, matching presentation.
pub(crate) fn compare(
    baseline: &RgbaImage,
    current: &RgbaImage,
    tolerance: VisualTolerance,
) -> (VisualDiffReport, RgbaImage) {
    let width = current.width();
    let height = current.height();
    if baseline.dimensions() != current.dimensions() {
        let report = VisualDiffReport {
            width,
            height,
            baseline_width: baseline.width(),
            baseline_height: baseline.height(),
            compared_pixels: 0,
            changed_pixels: 0,
            changed_pixel_ratio: 1.0,
            mean_absolute_error: 255.0,
            max_channel_error: 255,
            mismatch_bounds: None,
            tolerance,
            comparison_profile: "unspecified".to_string(),
            baseline_backend: String::new(),
            current_backend: String::new(),
            passed: false,
            reason: format!(
                "dimension mismatch: baseline {}×{}, current {}×{}",
                baseline.width(),
                baseline.height(),
                width,
                height
            ),
        };
        return (report, current.clone());
    }

    let mut diff = RgbaImage::new(width, height);
    let mut changed_pixels = 0_u64;
    let mut total_error = 0_u64;
    let mut max_channel_error = 0_u8;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    for y in 0..height {
        for x in 0..width {
            let expected = baseline.get_pixel(x, y).0;
            let actual = current.get_pixel(x, y).0;
            let delta = [
                expected[0].abs_diff(actual[0]),
                expected[1].abs_diff(actual[1]),
                expected[2].abs_diff(actual[2]),
            ];
            let pixel_max = *delta.iter().max().unwrap_or(&0);
            max_channel_error = max_channel_error.max(pixel_max);
            total_error += delta.iter().map(|value| u64::from(*value)).sum::<u64>();
            let changed = pixel_max > tolerance.channel_threshold;
            if changed {
                changed_pixels += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                diff.put_pixel(x, y, Rgba([255, pixel_max / 3, 24, 255]));
            } else {
                // Preserve faint luminance context so the red failure region is
                // easy to locate without confusing the diff for a screenshot.
                let luma = ((u16::from(actual[0]) + actual[1] as u16 + actual[2] as u16) / 9)
                    as u8;
                diff.put_pixel(x, y, Rgba([luma, luma, luma, 255]));
            }
        }
    }
    let compared_pixels = u64::from(width) * u64::from(height);
    let changed_pixel_ratio = if compared_pixels == 0 {
        0.0
    } else {
        changed_pixels as f32 / compared_pixels as f32
    };
    let mean_absolute_error = if compared_pixels == 0 {
        0.0
    } else {
        total_error as f32 / (compared_pixels * 3) as f32
    };
    let ratio_passed = changed_pixel_ratio <= tolerance.changed_pixel_ratio;
    let mean_passed = mean_absolute_error <= tolerance.mean_absolute_error;
    let passed = ratio_passed && mean_passed;
    let reason = match (ratio_passed, mean_passed) {
        (true, true) => "within configured tolerance".to_string(),
        (false, true) => format!(
            "changed-pixel ratio exceeds {:.3}%",
            tolerance.changed_pixel_ratio * 100.0
        ),
        (true, false) => format!(
            "mean error exceeds {:.3}",
            tolerance.mean_absolute_error
        ),
        (false, false) => format!(
            "changed-pixel ratio exceeds {:.3}% and mean error exceeds {:.3}",
            tolerance.changed_pixel_ratio * 100.0,
            tolerance.mean_absolute_error
        ),
    };
    let mismatch_bounds = if changed_pixels > 0 {
        Some(PixelBounds {
            x: min_x,
            y: min_y,
            width: max_x - min_x + 1,
            height: max_y - min_y + 1,
        })
    } else {
        None
    };
    (
        VisualDiffReport {
            width,
            height,
            baseline_width: baseline.width(),
            baseline_height: baseline.height(),
            compared_pixels,
            changed_pixels,
            changed_pixel_ratio,
            mean_absolute_error,
            max_channel_error,
            mismatch_bounds,
            tolerance,
            comparison_profile: "unspecified".to_string(),
            baseline_backend: String::new(),
            current_backend: String::new(),
            passed,
            reason,
        },
        diff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{MeshData, MeshHandle, Submesh, Vertex};
    use crate::platform::{Color, lock_platform_state, new_shared_platform_state};
    use crate::render3d::{Camera3D, Mat4, Mesh3DCommand, Vec3};
    use crate::renderer::{DrawCommand, SoftwareRenderer};

    fn representative_3d_frame() -> RgbaImage {
        let mesh = MeshHandle::new(
            MeshData::new(
                "visual parity triangle",
                vec![
                    Vertex::from_position([-0.8, -0.8, 0.0]),
                    Vertex::from_position([0.8, -0.8, 0.0]),
                    Vertex::from_position([0.0, 0.8, 0.0]),
                ],
                vec![0, 1, 2],
                vec![Submesh {
                    name: "triangle".into(),
                    first_index: 0,
                    index_count: 3,
                    material: None,
                }],
                Vec::new(),
                true,
            )
            .expect("mesh"),
        )
        .expect("mesh handle");
        let camera = Camera3D::default();
        let command = DrawCommand::Mesh3D(Mesh3DCommand {
            mesh,
            model: Mat4::translation(Vec3::new(0.0, 0.0, 2.0)),
            view_projection: camera.view_projection(1.0),
            camera_position: camera.position,
            tint: Color::rgba(210, 85, 40, 255),
            texture: None,
            materials: Vec::new(),
            shader: None,
            double_sided: true,
            casts_shadows: true,
            receives_shadows: true,
        });
        let platform = new_shared_platform_state();
        lock_platform_state(&platform).set_clear_color(Color::rgba(8, 12, 18, 255));
        let mut renderer = SoftwareRenderer::new(64, 64);
        renderer
            .render_commands(&platform, &[command])
            .expect("representative 3D frame");
        RgbaImage::from_raw(64, 64, renderer.pixels().to_vec()).expect("RGBA frame")
    }

    #[test]
    fn identical_real_3d_frames_pass_and_localized_change_reports_bounds() {
        let baseline = representative_3d_frame();
        let (same, _) = compare(&baseline, &baseline, VisualTolerance::default());
        assert!(same.passed);
        assert_eq!(same.changed_pixels, 0);

        let mut changed = baseline.clone();
        for y in 20..28 {
            for x in 30..38 {
                changed.put_pixel(x, y, Rgba([255, 0, 255, 255]));
            }
        }
        let (report, diff) = compare(&baseline, &changed, VisualTolerance::default());
        assert!(!report.passed);
        assert_eq!(
            report.mismatch_bounds,
            Some(PixelBounds {
                x: 30,
                y: 20,
                width: 8,
                height: 8,
            })
        );
        assert_eq!(diff.get_pixel(30, 20).0[0], 255);
    }

    #[test]
    fn threshold_allows_small_backend_noise_but_rejects_dimensions() {
        let baseline = RgbaImage::from_pixel(4, 3, Rgba([50, 80, 120, 255]));
        let noisy = RgbaImage::from_pixel(4, 3, Rgba([51, 79, 121, 255]));
        let (report, _) = compare(&baseline, &noisy, VisualTolerance::default());
        assert!(report.passed);
        let wrong_size = RgbaImage::new(5, 3);
        let (report, _) = compare(&baseline, &wrong_size, VisualTolerance::default());
        assert!(!report.passed);
        assert!(report.reason.contains("dimension mismatch"));
    }

    #[test]
    fn cross_backend_profile_allows_sparse_aa_edges_but_not_image_corruption() {
        let baseline = RgbaImage::from_pixel(100, 100, Rgba([20, 30, 40, 255]));
        let mut edge_coverage = baseline.clone();
        for x in 0..250 {
            edge_coverage.put_pixel(x % 100, x / 100, Rgba([29, 39, 49, 255]));
        }
        let (strict, _) = compare(&baseline, &edge_coverage, VisualTolerance::default());
        let (cross_backend, _) = compare(
            &baseline,
            &edge_coverage,
            VisualTolerance::cross_backend(),
        );
        assert!(!strict.passed);
        assert!(cross_backend.passed);

        let mut corrupted = baseline.clone();
        for y in 0..10 {
            for x in 0..10 {
                corrupted.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let (corrupted, _) = compare(
            &baseline,
            &corrupted,
            VisualTolerance::cross_backend(),
        );
        assert!(!corrupted.passed);
        assert!(corrupted.mean_absolute_error > corrupted.tolerance.mean_absolute_error);
    }
}
