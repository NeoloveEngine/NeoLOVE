//! Fixed-capacity 3D particle simulation.
//!
//! A particle component owns one native pool for its lifetime. The pool keeps
//! its allocation between frames, uses deterministic seeded random numbers,
//! and rejects emission once full. Rendering reads the live pool through a
//! lightweight shared handle and expands all particles into one billboard
//! batch per component.

use crate::assets::ImageHandle;
use crate::platform::Color;
use crate::render3d::{Mat4, Vec3};
use crate::renderer::TextureFilter;
use mlua::{UserData, UserDataMethods};
use std::sync::{Arc, Mutex};

pub(crate) const MAX_PARTICLES_PER_EMITTER: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EmissionShape3D {
    #[default]
    Point,
    Box,
    Sphere,
    Cone,
}

#[derive(Clone, Debug)]
pub(crate) struct ParticleConfig3D {
    pub max_particles: usize,
    pub playing: bool,
    pub looping: bool,
    pub duration: f32,
    pub emission_rate: f32,
    pub shape: EmissionShape3D,
    pub box_extents: Vec3,
    pub sphere_radius: f32,
    pub cone_angle_degrees: f32,
    pub cone_length: f32,
    pub direction: Vec3,
    pub spread_degrees: f32,
    pub lifetime_min: f32,
    pub lifetime_max: f32,
    pub speed_min: f32,
    pub speed_max: f32,
    pub gravity: Vec3,
    pub drag: f32,
    pub start_size: f32,
    pub end_size: f32,
    pub start_color: Color,
    pub end_color: Color,
    pub start_rotation_degrees: f32,
    pub angular_velocity_degrees: f32,
}

impl Default for ParticleConfig3D {
    fn default() -> Self {
        Self {
            max_particles: 1024,
            playing: true,
            looping: true,
            duration: 5.0,
            emission_rate: 24.0,
            shape: EmissionShape3D::Point,
            box_extents: Vec3::new(1.0, 1.0, 1.0),
            sphere_radius: 1.0,
            cone_angle_degrees: 30.0,
            cone_length: 1.0,
            direction: Vec3::new(0.0, 1.0, 0.0),
            spread_degrees: 12.0,
            lifetime_min: 1.0,
            lifetime_max: 2.0,
            speed_min: 1.0,
            speed_max: 3.0,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            drag: 0.0,
            start_size: 0.25,
            end_size: 0.0,
            start_color: Color::rgba(255, 190, 80, 255),
            end_color: Color::rgba(255, 70, 20, 0),
            start_rotation_degrees: 0.0,
            angular_velocity_degrees: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RenderParticle3D {
    pub position: Vec3,
    pub size: f32,
    pub rotation_degrees: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug)]
struct Particle3D {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    start_size: f32,
    end_size: f32,
    start_color: Color,
    end_color: Color,
    rotation_degrees: f32,
    angular_velocity_degrees: f32,
}

#[derive(Debug)]
struct ParticleEmitter3D {
    particles: Vec<Particle3D>,
    capacity: usize,
    playing: bool,
    elapsed: f32,
    emission_accumulator: f32,
    pending_emission: usize,
    seed: u32,
}

impl ParticleEmitter3D {
    fn new(capacity: usize, seed: u32) -> Self {
        let capacity = capacity.clamp(1, MAX_PARTICLES_PER_EMITTER);
        Self {
            particles: Vec::with_capacity(capacity),
            capacity,
            playing: true,
            elapsed: 0.0,
            emission_accumulator: 0.0,
            pending_emission: 0,
            seed: seed.max(1),
        }
    }

    fn random(&mut self) -> f32 {
        // xorshift32 is cheap, deterministic on every target, and sufficient
        // for visual sampling. A zero state is prevented by construction.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as f32 / u32::MAX as f32
    }

    fn random_range(&mut self, minimum: f32, maximum: f32) -> f32 {
        let (minimum, maximum) = if minimum <= maximum {
            (minimum, maximum)
        } else {
            (maximum, minimum)
        };
        minimum + (maximum - minimum) * self.random()
    }

    fn set_capacity(&mut self, requested: usize) {
        let requested = requested.clamp(1, MAX_PARTICLES_PER_EMITTER);
        if requested == self.capacity {
            return;
        }
        self.capacity = requested;
        self.particles.truncate(requested);
        if self.particles.capacity() < requested {
            self.particles
                .reserve_exact(requested.saturating_sub(self.particles.len()));
        }
    }

    fn play(&mut self) {
        self.playing = true;
    }

    fn pause(&mut self) {
        self.playing = false;
    }

    fn stop(&mut self) {
        self.playing = false;
        self.elapsed = 0.0;
        self.emission_accumulator = 0.0;
        self.pending_emission = 0;
        self.particles.clear();
    }

    fn emit(&mut self, count: usize) {
        self.pending_emission = self
            .pending_emission
            .saturating_add(count)
            .min(MAX_PARTICLES_PER_EMITTER);
    }

    fn step(&mut self, dt: f32, origin: Vec3, euler: Vec3, config: &ParticleConfig3D) {
        self.set_capacity(config.max_particles);
        // Direct edits to the public `playing` property remain authoritative,
        // while the control methods update that property and this state.
        self.playing = config.playing;
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.25)
        } else {
            0.0
        };
        let damping = (-config.drag.max(0.0) * dt).exp();
        let mut index = 0;
        while index < self.particles.len() {
            let particle = &mut self.particles[index];
            particle.age += dt;
            if particle.age >= particle.lifetime || !particle.age.is_finite() {
                self.particles.swap_remove(index);
                continue;
            }
            particle.velocity.x = (particle.velocity.x + config.gravity.x * dt) * damping;
            particle.velocity.y = (particle.velocity.y + config.gravity.y * dt) * damping;
            particle.velocity.z = (particle.velocity.z + config.gravity.z * dt) * damping;
            particle.position.x += particle.velocity.x * dt;
            particle.position.y += particle.velocity.y * dt;
            particle.position.z += particle.velocity.z * dt;
            particle.rotation_degrees += particle.angular_velocity_degrees * dt;
            index += 1;
        }

        let mut automatic = 0usize;
        if self.playing {
            self.elapsed += dt;
            let duration = config.duration.max(0.0);
            if duration > 0.0 && self.elapsed >= duration {
                if config.looping {
                    self.elapsed %= duration;
                } else {
                    self.playing = false;
                }
            }
            if self.playing {
                self.emission_accumulator += config.emission_rate.max(0.0) * dt;
                automatic = self.emission_accumulator.floor().max(0.0) as usize;
                self.emission_accumulator -= automatic as f32;
            }
        }
        let requested = automatic.saturating_add(std::mem::take(&mut self.pending_emission));
        let available = self.capacity.saturating_sub(self.particles.len());
        let spawn_count = requested.min(available);
        let rotation = Mat4::rotation_euler_degrees(euler);
        for _ in 0..spawn_count {
            self.spawn(origin, rotation, config);
        }
    }

    fn spawn(&mut self, origin: Vec3, rotation: Mat4, config: &ParticleConfig3D) {
        let base_direction = normalize(config.direction, Vec3::new(0.0, 1.0, 0.0));
        let (local_offset, local_direction) = match config.shape {
            EmissionShape3D::Point => (
                Vec3::ZERO,
                random_cone_direction(self, base_direction, config.spread_degrees),
            ),
            EmissionShape3D::Box => {
                let offset = Vec3::new(
                    self.random_range(-config.box_extents.x.abs(), config.box_extents.x.abs()),
                    self.random_range(-config.box_extents.y.abs(), config.box_extents.y.abs()),
                    self.random_range(-config.box_extents.z.abs(), config.box_extents.z.abs()),
                );
                (
                    offset,
                    random_cone_direction(self, base_direction, config.spread_degrees),
                )
            }
            EmissionShape3D::Sphere => {
                let direction = random_unit_vector(self);
                let radius = self.random().cbrt() * config.sphere_radius.max(0.0);
                (
                    mul(direction, radius),
                    random_cone_direction(self, direction, config.spread_degrees),
                )
            }
            EmissionShape3D::Cone => {
                let direction =
                    random_cone_direction(self, base_direction, config.cone_angle_degrees.abs());
                let distance = self.random().cbrt() * config.cone_length.max(0.0);
                (mul(direction, distance), direction)
            }
        };
        let world_offset = rotation.transform_direction(local_offset);
        let world_direction = normalize(
            rotation.transform_direction(local_direction),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let speed = self
            .random_range(config.speed_min, config.speed_max)
            .max(0.0);
        let lifetime = self
            .random_range(config.lifetime_min, config.lifetime_max)
            .max(0.001);
        self.particles.push(Particle3D {
            position: add(origin, world_offset),
            velocity: mul(world_direction, speed),
            age: 0.0,
            lifetime,
            start_size: config.start_size.max(0.0),
            end_size: config.end_size.max(0.0),
            start_color: config.start_color,
            end_color: config.end_color,
            rotation_degrees: config.start_rotation_degrees,
            angular_velocity_degrees: config.angular_velocity_degrees,
        });
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ParticleEmitterHandle(Arc<Mutex<ParticleEmitter3D>>);

impl ParticleEmitterHandle {
    pub(crate) fn new(capacity: usize, seed: u32) -> Self {
        Self(Arc::new(Mutex::new(ParticleEmitter3D::new(capacity, seed))))
    }

    pub(crate) fn play(&self) -> Result<(), String> {
        self.with_emitter_mut(|emitter| emitter.play())
    }

    pub(crate) fn pause(&self) -> Result<(), String> {
        self.with_emitter_mut(|emitter| emitter.pause())
    }

    pub(crate) fn stop(&self) -> Result<(), String> {
        self.with_emitter_mut(|emitter| emitter.stop())
    }

    pub(crate) fn emit(&self, count: usize) -> Result<(), String> {
        self.with_emitter_mut(|emitter| emitter.emit(count))
    }

    pub(crate) fn step(
        &self,
        dt: f32,
        origin: Vec3,
        euler: Vec3,
        config: &ParticleConfig3D,
    ) -> Result<(), String> {
        self.with_emitter_mut(|emitter| emitter.step(dt, origin, euler, config))
    }

    pub(crate) fn particle_count(&self) -> Result<usize, String> {
        self.0
            .lock()
            .map(|emitter| emitter.particles.len())
            .map_err(|_| "particle emitter lock poisoned".to_string())
    }

    pub(crate) fn is_playing(&self) -> Result<bool, String> {
        self.0
            .lock()
            .map(|emitter| emitter.playing)
            .map_err(|_| "particle emitter lock poisoned".to_string())
    }

    pub(crate) fn render_particles(&self) -> Result<Vec<RenderParticle3D>, String> {
        let emitter = self
            .0
            .lock()
            .map_err(|_| "particle emitter lock poisoned".to_string())?;
        let mut output = Vec::with_capacity(emitter.particles.len());
        for particle in &emitter.particles {
            let progress = (particle.age / particle.lifetime).clamp(0.0, 1.0);
            output.push(RenderParticle3D {
                position: particle.position,
                size: particle.start_size + (particle.end_size - particle.start_size) * progress,
                rotation_degrees: particle.rotation_degrees,
                color: lerp_color(particle.start_color, particle.end_color, progress),
            });
        }
        Ok(output)
    }

    fn with_emitter_mut(
        &self,
        operation: impl FnOnce(&mut ParticleEmitter3D),
    ) -> Result<(), String> {
        let mut emitter = self
            .0
            .lock()
            .map_err(|_| "particle emitter lock poisoned".to_string())?;
        operation(&mut emitter);
        Ok(())
    }
}

impl UserData for ParticleEmitterHandle {
    fn add_methods<M: UserDataMethods<Self>>(_methods: &mut M) {}
}

#[derive(Clone, Debug)]
pub(crate) struct ParticleSystem3DCommand {
    pub emitter: ParticleEmitterHandle,
    pub view_projection: Mat4,
    pub camera_euler: Vec3,
    pub texture: Option<ImageHandle>,
    pub filter: TextureFilter,
}

fn add(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x + right.x, left.y + right.y, left.z + right.z)
}

fn mul(value: Vec3, amount: f32) -> Vec3 {
    Vec3::new(value.x * amount, value.y * amount, value.z * amount)
}

fn dot(left: Vec3, right: Vec3) -> f32 {
    left.x * right.x + left.y * right.y + left.z * right.z
}

fn cross(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(
        left.y * right.z - left.z * right.y,
        left.z * right.x - left.x * right.z,
        left.x * right.y - left.y * right.x,
    )
}

fn normalize(value: Vec3, fallback: Vec3) -> Vec3 {
    let length_squared = dot(value, value);
    if length_squared <= f32::EPSILON || !length_squared.is_finite() {
        fallback
    } else {
        mul(value, length_squared.sqrt().recip())
    }
}

fn random_unit_vector(emitter: &mut ParticleEmitter3D) -> Vec3 {
    let z = emitter.random() * 2.0 - 1.0;
    let angle = emitter.random() * std::f32::consts::TAU;
    let radius = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(radius * angle.cos(), z, radius * angle.sin())
}

fn random_cone_direction(emitter: &mut ParticleEmitter3D, axis: Vec3, angle_degrees: f32) -> Vec3 {
    let axis = normalize(axis, Vec3::new(0.0, 1.0, 0.0));
    let maximum = angle_degrees.clamp(0.0, 179.0).to_radians();
    if maximum <= f32::EPSILON {
        return axis;
    }
    let reference = if axis.y.abs() < 0.999 {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let tangent = normalize(cross(reference, axis), Vec3::new(1.0, 0.0, 0.0));
    let bitangent = cross(axis, tangent);
    let cos_theta = 1.0 - emitter.random() * (1.0 - maximum.cos());
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let azimuth = emitter.random() * std::f32::consts::TAU;
    normalize(
        add(
            mul(axis, cos_theta),
            add(
                mul(tangent, sin_theta * azimuth.cos()),
                mul(bitangent, sin_theta * azimuth.sin()),
            ),
        ),
        axis,
    )
}

fn lerp_color(start: Color, end: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |start: u8, end: u8| {
        (start as f32 + (end as f32 - start as f32) * amount)
            .clamp(0.0, 255.0)
            .round() as u8
    };
    Color::rgba(
        channel(start.r, end.r),
        channel(start.g, end.g),
        channel(start.b, end.b),
        channel(start.a, end.a),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn still_config(max_particles: usize) -> ParticleConfig3D {
        ParticleConfig3D {
            max_particles,
            playing: false,
            lifetime_min: 10.0,
            lifetime_max: 10.0,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: Vec3::ZERO,
            ..ParticleConfig3D::default()
        }
    }

    #[test]
    fn explicit_emission_is_bounded_by_fixed_capacity() {
        let emitter = ParticleEmitterHandle::new(4, 7);
        emitter.emit(100).expect("queue emission");
        emitter
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &still_config(4))
            .expect("step");
        assert_eq!(emitter.particle_count().expect("count"), 4);
        assert!(emitter.render_particles().expect("render particles").len() <= 4);
    }

    #[test]
    fn stop_reuses_pool_and_clears_particles() {
        let emitter = ParticleEmitterHandle::new(8, 9);
        emitter.emit(5).expect("emit");
        emitter
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &still_config(8))
            .expect("step");
        emitter.stop().expect("stop");
        assert_eq!(emitter.particle_count().expect("count"), 0);
        emitter.emit(3).expect("emit after stop");
        emitter
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &still_config(8))
            .expect("step after stop");
        assert_eq!(emitter.particle_count().expect("count"), 3);
    }

    #[test]
    fn identical_seeds_produce_identical_sphere_particles() {
        let config = ParticleConfig3D {
            shape: EmissionShape3D::Sphere,
            sphere_radius: 3.0,
            ..still_config(16)
        };
        let first = ParticleEmitterHandle::new(16, 1234);
        let second = ParticleEmitterHandle::new(16, 1234);
        first.emit(12).expect("first emit");
        second.emit(12).expect("second emit");
        first
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &config)
            .expect("first step");
        second
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &config)
            .expect("second step");
        assert_eq!(
            first.render_particles().expect("first snapshot"),
            second.render_particles().expect("second snapshot")
        );
    }

    #[test]
    fn gravity_drag_size_and_color_are_updated_without_growing_pool() {
        let emitter = ParticleEmitterHandle::new(2, 42);
        let config = ParticleConfig3D {
            playing: false,
            lifetime_min: 2.0,
            lifetime_max: 2.0,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: Vec3::new(0.0, -4.0, 0.0),
            start_size: 2.0,
            end_size: 0.0,
            start_color: Color::rgba(255, 0, 0, 255),
            end_color: Color::rgba(0, 0, 255, 0),
            max_particles: 2,
            ..ParticleConfig3D::default()
        };
        emitter.emit(1).expect("emit");
        emitter
            .step(0.0, Vec3::ZERO, Vec3::ZERO, &config)
            .expect("spawn");
        emitter
            .step(1.0, Vec3::ZERO, Vec3::ZERO, &config)
            .expect("advance");
        let particle = emitter.render_particles().expect("snapshot")[0];
        assert!(particle.position.y < 0.0);
        assert!((particle.size - 1.75).abs() < 0.001);
        assert!(particle.color.r < 255 && particle.color.b > 0);
    }
}
