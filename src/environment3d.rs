//! Runtime 3D environment backgrounds.
//!
//! Environments are deliberately independent of scene geometry: they fill the
//! framebuffer before depth-tested 3D commands and therefore never consume or
//! interfere with depth. Solid and vertical-gradient modes are allocation
//! free. Equirectangular images are sampled directly from their revisioned
//! image handle so live texture edits appear on the next frame.

use crate::assets::{CubemapHandle, ImageHandle};
use crate::platform::Color;
use crate::render3d::{Camera3D, Mat4, Vec3};
use image::RgbaImage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EnvironmentMode3D {
    Solid,
    #[default]
    Gradient,
    Equirectangular,
    Cubemap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FogMode3D {
    #[default]
    Linear,
    Exponential,
    ExponentialSquared,
}

/// Camera-distance fog shared by Scene View and every runtime backend.
///
/// The returned amount is the fraction of the shaded surface replaced by the
/// fog color. Keeping this policy backend-neutral prevents slightly different
/// start/end sanitization from producing obvious editor/runtime bands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fog3D {
    pub enabled: bool,
    pub mode: FogMode3D,
    pub color: Color,
    pub start_distance: f32,
    pub end_distance: f32,
    pub density: f32,
}

impl Default for Fog3D {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: FogMode3D::Linear,
            color: Color::rgba(110, 125, 145, 255),
            start_distance: 10.0,
            end_distance: 100.0,
            density: 0.02,
        }
    }
}

impl Fog3D {
    pub(crate) fn sanitized(self) -> Self {
        let start_distance = if self.start_distance.is_finite() {
            self.start_distance.max(0.0)
        } else {
            10.0
        };
        let end_distance = if self.end_distance.is_finite() {
            self.end_distance.max(start_distance + 0.0001)
        } else {
            100.0f32.max(start_distance + 0.0001)
        };
        let density = if self.density.is_finite() {
            self.density.max(0.0)
        } else {
            0.02
        };
        Self {
            start_distance,
            end_distance,
            density,
            ..self
        }
    }

    pub(crate) fn amount_at_distance(self, distance: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let fog = self.sanitized();
        let distance = if distance.is_finite() {
            distance.max(0.0)
        } else {
            fog.end_distance
        };
        match fog.mode {
            FogMode3D::Linear => ((distance - fog.start_distance)
                / (fog.end_distance - fog.start_distance))
                .clamp(0.0, 1.0),
            FogMode3D::Exponential => 1.0 - (-fog.density * distance).exp(),
            FogMode3D::ExponentialSquared => {
                let scaled = fog.density * distance;
                1.0 - (-(scaled * scaled)).exp()
            }
        }
        .clamp(0.0, 1.0)
    }

    pub(crate) fn amount(self, camera: Vec3, world_position: Vec3) -> f32 {
        self.amount_at_distance(world_position.sub(camera).length_squared().sqrt())
    }

    pub(crate) fn color_channels(self) -> [f32; 4] {
        [
            self.color.r as f32 / 255.0,
            self.color.g as f32 / 255.0,
            self.color.b as f32 / 255.0,
            self.color.a as f32 / 255.0,
        ]
    }
}

/// World-space analytic ambient occlusion. Meshes contribute transformed
/// bounds as conservative occluders; the bounded runtime evaluator darkens
/// nearby receiver-facing surfaces without depending on viewport resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AmbientOcclusion3D {
    pub enabled: bool,
    pub radius: f32,
    pub intensity: f32,
    pub bias: f32,
}

impl Default for AmbientOcclusion3D {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 2.5,
            intensity: 0.65,
            bias: 0.025,
        }
    }
}

impl AmbientOcclusion3D {
    pub(crate) fn sanitized(self) -> Self {
        Self {
            radius: if self.radius.is_finite() {
                self.radius.clamp(0.001, 10_000.0)
            } else {
                2.5
            },
            intensity: if self.intensity.is_finite() {
                self.intensity.clamp(0.0, 1.0)
            } else {
                0.65
            },
            bias: if self.bias.is_finite() {
                self.bias.clamp(0.0, 1_000.0)
            } else {
                0.025
            },
            ..self
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Environment3D {
    pub enabled: bool,
    pub mode: EnvironmentMode3D,
    pub solid: Color,
    pub top: Color,
    pub bottom: Color,
    pub equirectangular: Option<ImageHandle>,
    pub cubemap: Option<CubemapHandle>,
    /// Yaw applied to the sampled panorama, in authored degrees.
    pub rotation_degrees: f32,
    pub intensity: f32,
    pub fog: Fog3D,
    pub ambient_occlusion: AmbientOcclusion3D,
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
            cubemap: None,
            rotation_degrees: 0.0,
            intensity: 1.0,
            fog: Fog3D::default(),
            ambient_occlusion: AmbientOcclusion3D::default(),
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
    cubemap_image_ids: [usize; 6],
    cubemap_image_revisions: [u64; 6],
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
    let (cubemap_image_ids, cubemap_image_revisions) = environment
        .cubemap
        .as_ref()
        .and_then(|cubemap| cubemap.snapshot().ok())
        .map(|snapshot| (snapshot.identities, snapshot.revisions))
        .unwrap_or(([0; 6], [0; 6]));
    SoftwareEnvironmentCacheKey {
        dimensions: [width, height],
        mode: match environment.mode {
            EnvironmentMode3D::Solid => 0,
            EnvironmentMode3D::Gradient => 1,
            EnvironmentMode3D::Equirectangular => 2,
            EnvironmentMode3D::Cubemap => 3,
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
        cubemap_image_ids,
        cubemap_image_revisions,
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

pub(crate) fn cubemap_face_uv(direction: Vec3, rotation_degrees: f32) -> (usize, [f32; 2]) {
    let yaw = rotation_degrees.to_radians();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let x = direction.x * cos_yaw - direction.z * sin_yaw;
    let y = direction.y;
    let z = direction.x * sin_yaw + direction.z * cos_yaw;
    let ax = x.abs();
    let ay = y.abs();
    let az = z.abs();
    let (face, sc, tc, major) = if ax >= ay && ax >= az {
        if x >= 0.0 {
            (0, -z, -y, ax)
        } else {
            (1, z, -y, ax)
        }
    } else if ay >= az {
        if y >= 0.0 {
            (2, x, z, ay)
        } else {
            (3, x, -z, ay)
        }
    } else if z >= 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    };
    let inverse_major = major.max(f32::EPSILON).recip();
    (
        face,
        [
            (sc * inverse_major * 0.5 + 0.5).clamp(0.0, 1.0),
            (tc * inverse_major * 0.5 + 0.5).clamp(0.0, 1.0),
        ],
    )
}

fn sample_cubemap(
    faces: &[std::sync::Arc<RgbaImage>; 6],
    direction: Vec3,
    rotation_degrees: f32,
) -> Color {
    let (face, uv) = cubemap_face_uv(direction, rotation_degrees);
    let image = &faces[face];
    if image.width() == 0 || image.height() == 0 {
        return Color::rgba(0, 0, 0, 255);
    }
    let x = (uv[0] * image.width().saturating_sub(1) as f32).round() as u32;
    let y = (uv[1] * image.height().saturating_sub(1) as f32).round() as u32;
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

fn fill_cubemap(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    faces: &[std::sync::Arc<RgbaImage>; 6],
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
                sample_cubemap(faces, direction, rotation_degrees),
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
        EnvironmentMode3D::Cubemap => {
            let rendered = environment.cubemap.as_ref().is_some_and(|cubemap| {
                cubemap.snapshot().is_ok_and(|snapshot| {
                    fill_cubemap(
                        pixels,
                        width,
                        height,
                        &snapshot.faces,
                        camera,
                        environment.rotation_degrees,
                        environment.intensity,
                    );
                    true
                })
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
    fn fog_modes_share_sanitized_distance_policy() {
        let linear = Fog3D {
            enabled: true,
            start_distance: 10.0,
            end_distance: 30.0,
            ..Fog3D::default()
        };
        assert_eq!(linear.amount_at_distance(5.0), 0.0);
        assert!((linear.amount_at_distance(20.0) - 0.5).abs() < 0.0001);
        assert_eq!(linear.amount_at_distance(40.0), 1.0);

        let exponential = Fog3D {
            mode: FogMode3D::Exponential,
            density: 0.1,
            ..linear
        };
        let exponential_squared = Fog3D {
            mode: FogMode3D::ExponentialSquared,
            ..exponential
        };
        assert!(exponential.amount_at_distance(20.0) > exponential.amount_at_distance(10.0));
        assert!(exponential_squared.amount_at_distance(20.0) > 0.0);
        assert_eq!(Fog3D::default().amount_at_distance(f32::INFINITY), 0.0);

        let invalid = Fog3D {
            enabled: true,
            start_distance: f32::NAN,
            end_distance: -5.0,
            density: f32::INFINITY,
            ..Fog3D::default()
        }
        .sanitized();
        assert_eq!(invalid.start_distance, 10.0);
        assert!(invalid.end_distance > invalid.start_distance);
        assert_eq!(invalid.density, 0.02);
    }

    #[test]
    fn ambient_occlusion_settings_are_bounded_and_disable_cleanly() {
        let invalid = AmbientOcclusion3D {
            enabled: true,
            radius: f32::NAN,
            intensity: 4.0,
            bias: -8.0,
        }
        .sanitized();
        assert_eq!(invalid.radius, 2.5);
        assert_eq!(invalid.intensity, 1.0);
        assert_eq!(invalid.bias, 0.0);
        assert!(!AmbientOcclusion3D::default().enabled);
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

    #[test]
    fn cubemap_uses_documented_face_order_and_camera_yaw() {
        let faces = std::array::from_fn(|index| {
            ImageHandle::from_rgba_image(RgbaImage::from_pixel(
                2,
                2,
                Rgba([index as u8 * 35, 0, 0, 255]),
            ))
        });
        let environment = Environment3D {
            enabled: true,
            mode: EnvironmentMode3D::Cubemap,
            cubemap: Some(CubemapHandle::new(faces).expect("valid cubemap")),
            ..Environment3D::default()
        };
        let mut facing_negative_z = vec![0; 4];
        render_software_background(
            &environment,
            Camera3D::default(),
            1,
            1,
            &mut facing_negative_z,
            Color::rgba(0, 0, 0, 255),
        );
        assert_eq!(facing_negative_z[0], 5 * 35);

        let mut camera = Camera3D::default();
        camera.euler.y = 180.0;
        let mut facing_positive_z = vec![0; 4];
        render_software_background(
            &environment,
            camera,
            1,
            1,
            &mut facing_positive_z,
            Color::rgba(0, 0, 0, 255),
        );
        assert_eq!(facing_positive_z[0], 4 * 35);
    }
}
