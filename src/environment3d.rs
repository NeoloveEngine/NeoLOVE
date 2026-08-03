//! Runtime 3D environment backgrounds.
//!
//! Environments are deliberately independent of scene geometry: they fill the
//! framebuffer before depth-tested 3D commands and therefore never consume or
//! interfere with depth. Solid and vertical-gradient modes are allocation
//! free. Equirectangular images are sampled directly from their revisioned
//! image handle so live texture edits appear on the next frame.

use crate::assets::ImageHandle;
use crate::platform::Color;
use crate::render3d::{Camera3D, Mat4, Vec3};
use image::RgbaImage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EnvironmentMode3D {
    Solid,
    #[default]
    Gradient,
    Equirectangular,
}

#[derive(Clone, Debug)]
pub(crate) struct Environment3D {
    pub enabled: bool,
    pub mode: EnvironmentMode3D,
    pub solid: Color,
    pub top: Color,
    pub bottom: Color,
    pub equirectangular: Option<ImageHandle>,
    /// Yaw applied to the sampled panorama, in authored degrees.
    pub rotation_degrees: f32,
    pub intensity: f32,
}

impl Default for Environment3D {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: EnvironmentMode3D::Gradient,
            solid: Color::rgba(20, 24, 32, 255),
            top: Color::rgba(30, 47, 78, 255),
            bottom: Color::rgba(8, 10, 16, 255),
            equirectangular: None,
            rotation_degrees: 0.0,
            intensity: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SoftwareEnvironmentCacheKey {
    dimensions: [usize; 2],
    mode: u8,
    enabled: bool,
    solid: [u8; 4],
    top: [u8; 4],
    bottom: [u8; 4],
    fallback: [u8; 4],
    intensity: u32,
    environment_rotation: u32,
    camera_euler: [u32; 3],
    camera_fov: u32,
    image_id: usize,
    image_revision: u64,
}

pub(crate) fn software_cache_key(
    environment: &Environment3D,
    camera: Camera3D,
    width: usize,
    height: usize,
    fallback: Color,
) -> SoftwareEnvironmentCacheKey {
    let color = |value: Color| [value.r, value.g, value.b, value.a];
    let (image_id, image_revision) = environment
        .equirectangular
        .as_ref()
        .map(|image| (image.id().unwrap_or(0), image.revision().unwrap_or(0)))
        .unwrap_or((0, 0));
    SoftwareEnvironmentCacheKey {
        dimensions: [width, height],
        mode: match environment.mode {
            EnvironmentMode3D::Solid => 0,
            EnvironmentMode3D::Gradient => 1,
            EnvironmentMode3D::Equirectangular => 2,
        },
        enabled: environment.enabled,
        solid: color(environment.solid),
        top: color(environment.top),
        bottom: color(environment.bottom),
        fallback: color(fallback),
        intensity: environment.intensity.to_bits(),
        environment_rotation: environment.rotation_degrees.to_bits(),
        camera_euler: [
            camera.euler.x.to_bits(),
            camera.euler.y.to_bits(),
            camera.euler.z.to_bits(),
        ],
        camera_fov: camera.fov.to_bits(),
        image_id,
        image_revision,
    }
}

fn scale_color(color: Color, intensity: f32) -> Color {
    let intensity = if intensity.is_finite() {
        intensity.max(0.0)
    } else {
        1.0
    };
    Color::rgba(
        (color.r as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        (color.g as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        (color.b as f32 * intensity).clamp(0.0, 255.0).round() as u8,
        color.a,
    )
}

fn lerp_color(top: Color, bottom: Color, amount: f32, intensity: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| {
        (a as f32 + (b as f32 - a as f32) * amount)
            .clamp(0.0, 255.0)
            .round() as u8
    };
    scale_color(
        Color::rgba(
            mix(top.r, bottom.r),
            mix(top.g, bottom.g),
            mix(top.b, bottom.b),
            mix(top.a, bottom.a),
        ),
        intensity,
    )
}

fn write_color(pixel: &mut [u8], color: Color) {
    pixel[0] = color.r;
    pixel[1] = color.g;
    pixel[2] = color.b;
    pixel[3] = color.a;
}

fn fill_solid(pixels: &mut [u8], color: Color) {
    for pixel in pixels.chunks_exact_mut(4) {
        write_color(pixel, color);
    }
}

fn fill_gradient(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    top: Color,
    bottom: Color,
    intensity: f32,
) {
    let denominator = height.saturating_sub(1).max(1) as f32;
    for y in 0..height {
        let color = lerp_color(top, bottom, y as f32 / denominator, intensity);
        let row = &mut pixels[y * width * 4..(y + 1) * width * 4];
        fill_solid(row, color);
    }
}

fn sample_panorama(image: &RgbaImage, direction: Vec3, rotation_degrees: f32) -> Color {
    if image.width() == 0 || image.height() == 0 {
        return Color::rgba(0, 0, 0, 255);
    }
    let yaw = rotation_degrees.to_radians();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let rotated_x = direction.x * cos_yaw - direction.z * sin_yaw;
    let rotated_z = direction.x * sin_yaw + direction.z * cos_yaw;
    let length = (rotated_x * rotated_x + direction.y * direction.y + rotated_z * rotated_z)
        .sqrt()
        .max(f32::EPSILON);
    let normalized_y = (direction.y / length).clamp(-1.0, 1.0);
    let u = (rotated_z.atan2(rotated_x) / std::f32::consts::TAU + 0.5).rem_euclid(1.0);
    let v = (0.5 - normalized_y.asin() / std::f32::consts::PI).clamp(0.0, 1.0);
    let x = (u * image.width() as f32).floor() as u32 % image.width();
    let y = (v * image.height().saturating_sub(1) as f32).round() as u32;
    let pixel = image.get_pixel(x, y).0;
    Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3])
}

fn fill_equirectangular(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    image: &RgbaImage,
    camera: Camera3D,
    rotation_degrees: f32,
    intensity: f32,
) {
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let half_height = (camera.fov.clamp(1.0, 179.0).to_radians() * 0.5).tan();
    let half_width = half_height * aspect;
    let rotation = Mat4::rotation_euler_degrees(camera.euler);
    let right = rotation.transform_direction(Vec3::new(1.0, 0.0, 0.0));
    let up = rotation.transform_direction(Vec3::new(0.0, 1.0, 0.0));
    let forward = rotation.transform_direction(Vec3::new(0.0, 0.0, -1.0));
    let inverse_width = width.max(1) as f32;
    let inverse_height = height.max(1) as f32;

    for y in 0..height {
        let camera_y = (1.0 - (y as f32 + 0.5) * 2.0 / inverse_height) * half_height;
        for x in 0..width {
            let camera_x = ((x as f32 + 0.5) * 2.0 / inverse_width - 1.0) * half_width;
            let direction = Vec3::new(
                forward.x + right.x * camera_x + up.x * camera_y,
                forward.y + right.y * camera_x + up.y * camera_y,
                forward.z + right.z * camera_x + up.z * camera_y,
            );
            let color = scale_color(
                sample_panorama(image, direction, rotation_degrees),
                intensity,
            );
            let offset = (y * width + x) * 4;
            write_color(&mut pixels[offset..offset + 4], color);
        }
    }
}

/// Fill a software framebuffer with the selected environment. Invalid or
/// unloaded panoramas fall back to the configured gradient rather than failing
/// the entire game frame.
pub(crate) fn render_software_background(
    environment: &Environment3D,
    camera: Camera3D,
    width: usize,
    height: usize,
    pixels: &mut [u8],
    fallback: Color,
) {
    if width == 0 || height == 0 || pixels.len() < width.saturating_mul(height).saturating_mul(4) {
        return;
    }
    if !environment.enabled {
        fill_solid(pixels, fallback);
        return;
    }
    match environment.mode {
        EnvironmentMode3D::Solid => {
            fill_solid(
                pixels,
                scale_color(environment.solid, environment.intensity),
            );
        }
        EnvironmentMode3D::Gradient => fill_gradient(
            pixels,
            width,
            height,
            environment.top,
            environment.bottom,
            environment.intensity,
        ),
        EnvironmentMode3D::Equirectangular => {
            let rendered = environment.equirectangular.as_ref().is_some_and(|image| {
                image
                    .with_image(|image| {
                        fill_equirectangular(
                            pixels,
                            width,
                            height,
                            image,
                            camera,
                            environment.rotation_degrees,
                            environment.intensity,
                        );
                    })
                    .is_ok()
            });
            if !rendered {
                fill_gradient(
                    pixels,
                    width,
                    height,
                    environment.top,
                    environment.bottom,
                    environment.intensity,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn gradient_fills_top_and_bottom_without_touching_depth() {
        let environment = Environment3D {
            enabled: true,
            mode: EnvironmentMode3D::Gradient,
            top: Color::rgba(10, 20, 30, 255),
            bottom: Color::rgba(110, 120, 130, 255),
            ..Environment3D::default()
        };
        let mut pixels = vec![0; 2 * 3 * 4];
        render_software_background(
            &environment,
            Camera3D::default(),
            2,
            3,
            &mut pixels,
            Color::rgba(0, 0, 0, 255),
        );
        assert_eq!(&pixels[0..4], &[10, 20, 30, 255]);
        assert_eq!(&pixels[4 * 4..5 * 4], &[110, 120, 130, 255]);
    }

    #[test]
    fn disabled_environment_uses_platform_clear_color() {
        let mut pixels = vec![0; 8];
        render_software_background(
            &Environment3D::default(),
            Camera3D::default(),
            2,
            1,
            &mut pixels,
            Color::rgba(7, 8, 9, 10),
        );
        assert_eq!(pixels, vec![7, 8, 9, 10, 7, 8, 9, 10]);
    }

    #[test]
    fn panorama_sampling_tracks_camera_yaw() {
        let panorama = RgbaImage::from_fn(4, 2, |x, _| Rgba([x as u8 * 50, 0, 0, 255]));
        let environment = Environment3D {
            enabled: true,
            mode: EnvironmentMode3D::Equirectangular,
            equirectangular: Some(ImageHandle::from_rgba_image(panorama)),
            ..Environment3D::default()
        };
        let mut facing_forward = vec![0; 4];
        render_software_background(
            &environment,
            Camera3D::default(),
            1,
            1,
            &mut facing_forward,
            Color::rgba(0, 0, 0, 255),
        );
        let mut camera = Camera3D::default();
        camera.euler.y = 180.0;
        let mut facing_backward = vec![0; 4];
        render_software_background(
            &environment,
            camera,
            1,
            1,
            &mut facing_backward,
            Color::rgba(0, 0, 0, 255),
        );
        assert_ne!(facing_forward[0], facing_backward[0]);
    }
}
