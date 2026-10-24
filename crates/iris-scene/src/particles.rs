//! Fixed-timestep particle pool.
//!
//! The pool is allocated once. Spawn never grows it past [`MAX_PARTICLES`].

use serde_json::{Map, Value};

use crate::error::Error;
use crate::instance::{quad_instance, ParticleInstance};

/// Hard cap for one emitter.
pub const MAX_PARTICLES: usize = 8_000;
/// Physics step. Frame stutter is subdivided, then leftover time is dropped.
pub const FIXED_DT: f32 = 1.0 / 60.0;
const MAX_STEPS: u32 = 5;

/// Emitter settings parsed from a Wallpaper Engine particle file or a flat object.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleDef {
    /// Particles per second.
    pub rate: f32,
    /// Lifetime in seconds.
    pub lifetime: f32,
    /// Initial speed in pixels per second.
    pub speed: f32,
    /// Acceleration in pixels per second squared.
    pub gravity: [f32; 3],
    /// Sideways acceleration scale.
    pub turbulence: f32,
    /// Size at birth, in pixels.
    pub start_size: f32,
    /// Size at death, in pixels.
    pub end_size: f32,
    /// Straight RGBA at birth.
    pub start_color: [f32; 4],
    /// Straight RGBA at death.
    pub end_color: [f32; 4],
    /// Requested pool size before the hard cap.
    pub capacity: usize,
}

impl Default for ParticleDef {
    fn default() -> Self {
        Self {
            rate: 20.0,
            lifetime: 1.5,
            speed: 40.0,
            gravity: [0.0, -30.0, 0.0],
            turbulence: 8.0,
            start_size: 12.0,
            end_size: 2.0,
            start_color: [1.0, 1.0, 1.0, 1.0],
            end_color: [1.0, 1.0, 1.0, 0.0],
            capacity: 512,
        }
    }
}

impl ParticleDef {
    /// Parse a particle JSON document. Missing fields keep the defaults.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|err| Error::Message(format!("particle json: {err}")))?;
        let Value::Object(map) = value else {
            return Err(Error::Message("particle json must be an object".to_owned()));
        };
        Ok(Self::from_map(&map))
    }

    fn from_map(map: &Map<String, Value>) -> Self {
        let mut def = Self::default();
        if let Some(rate) = number(map.get("rate")) {
            def.rate = rate.max(0.0);
        }
        if let Some(life) = number(map.get("lifetime")) {
            def.lifetime = life.max(0.05);
        }
        if let Some(speed) = number(map.get("speed")) {
            def.speed = speed;
        }
        if let Some(gravity) = vec3(map.get("gravity")) {
            def.gravity = gravity;
        }
        if let Some(turbulence) = number(map.get("turbulence")) {
            def.turbulence = turbulence;
        }
        if let Some(size) = number(map.get("start_size")).or_else(|| number(map.get("startsize"))) {
            def.start_size = size.max(0.0);
        }
        if let Some(size) = number(map.get("end_size")).or_else(|| number(map.get("endsize"))) {
            def.end_size = size.max(0.0);
        }
        if let Some(color) =
            color4(map.get("start_color")).or_else(|| color4(map.get("startcolor")))
        {
            def.start_color = color;
        }
        if let Some(color) = color4(map.get("end_color")).or_else(|| color4(map.get("endcolor"))) {
            def.end_color = color;
        }
        if let Some(capacity) = number(map.get("capacity")) {
            def.capacity = capacity as usize;
        }
        apply_wallpaper_engine(map, &mut def);
        def.capacity = def.capacity.clamp(1, MAX_PARTICLES);
        def.rate = def.rate.max(0.0);
        def
    }
}

fn apply_wallpaper_engine(map: &Map<String, Value>, def: &mut ParticleDef) {
    if let Some(Value::Array(emitters)) = map.get("emitter") {
        for emitter in emitters {
            let Some(object) = emitter.as_object() else {
                continue;
            };
            if let Some(rate) = number(object.get("rate")) {
                def.rate = rate.max(0.0);
            }
        }
    }
    if let Some(Value::Array(initializers)) = map.get("initializer") {
        for item in initializers {
            let Some(object) = item.as_object() else {
                continue;
            };
            let name = object.get("name").and_then(Value::as_str).unwrap_or("");
            match name {
                "lifetimerandom" => {
                    let min = number(object.get("min")).unwrap_or(def.lifetime);
                    let max = number(object.get("max")).unwrap_or(min);
                    def.lifetime = ((min + max) * 0.5).max(0.05);
                }
                "sizerandom" => {
                    def.start_size = number(object.get("max"))
                        .or_else(|| number(object.get("min")))
                        .unwrap_or(def.start_size)
                        .max(0.0);
                    def.end_size = def.start_size;
                }
                "velocityrandom" => {
                    let min = number(object.get("speedmin")).unwrap_or(def.speed);
                    let max = number(object.get("speedmax")).unwrap_or(min);
                    def.speed = (min + max) * 0.5;
                }
                "colorrandom" => {
                    if let Some(color) = color4(object.get("min")) {
                        def.start_color = color;
                        def.end_color = color;
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(Value::Array(operators)) = map.get("operator") {
        for item in operators {
            let Some(object) = item.as_object() else {
                continue;
            };
            let name = object.get("name").and_then(Value::as_str).unwrap_or("");
            if name == "movement" {
                if let Some(gravity) = vec3(object.get("gravity")) {
                    def.gravity = gravity;
                }
            }
            if name == "turbulence" {
                if let Some(scale) =
                    number(object.get("scale")).or_else(|| number(object.get("speed")))
                {
                    def.turbulence = scale;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Particle {
    active: bool,
    pos: [f32; 3],
    vel: [f32; 3],
    age: f32,
    life: f32,
    seed: f32,
}

/// One preallocated emitter.
#[derive(Debug, Clone)]
pub struct Emitter {
    def: ParticleDef,
    particles: Vec<Particle>,
    accum: f32,
    spawn_debt: f32,
    rng: u32,
    cursor: usize,
}

impl Emitter {
    /// Allocate the pool. The capacity is clamped to [`MAX_PARTICLES`].
    pub fn new(def: ParticleDef) -> Self {
        let capacity = def.capacity.clamp(1, MAX_PARTICLES);
        let particles = (0..capacity)
            .map(|index| Particle {
                active: false,
                pos: [0.0; 3],
                vel: [0.0; 3],
                age: 0.0,
                life: def.lifetime,
                seed: index as f32,
            })
            .collect();
        Self {
            def,
            particles,
            accum: 0.0,
            spawn_debt: 0.0,
            rng: 0x1234_5678,
            cursor: 0,
        }
    }

    /// Pool length after the hard cap.
    pub fn capacity(&self) -> usize {
        self.particles.len()
    }

    /// Active particles.
    pub fn alive(&self) -> usize {
        self.particles
            .iter()
            .filter(|particle| particle.active)
            .count()
    }

    /// Advance by `frame_dt` seconds using [`FIXED_DT`] substeps.
    pub fn step(&mut self, frame_dt: f32, origin: [f32; 2]) {
        if !frame_dt.is_finite() || frame_dt <= 0.0 {
            return;
        }
        self.accum += frame_dt.min(0.25);
        let mut steps = 0;
        while self.accum >= FIXED_DT && steps < MAX_STEPS {
            self.accum -= FIXED_DT;
            self.integrate(origin);
            steps += 1;
        }
        if steps == MAX_STEPS {
            self.accum = 0.0;
        }
    }

    /// Append live quads.
    pub fn push_instances(&self, out: &mut Vec<ParticleInstance>) {
        for particle in &self.particles {
            if !particle.active {
                continue;
            }
            let t = if particle.life <= f32::EPSILON {
                1.0
            } else {
                (particle.age / particle.life).clamp(0.0, 1.0)
            };
            let size = lerp(self.def.start_size, self.def.end_size, t).max(0.0);
            let color = [
                lerp(self.def.start_color[0], self.def.end_color[0], t),
                lerp(self.def.start_color[1], self.def.end_color[1], t),
                lerp(self.def.start_color[2], self.def.end_color[2], t),
                lerp(self.def.start_color[3], self.def.end_color[3], t),
            ];
            out.push(quad_instance(
                particle.pos[0],
                particle.pos[1],
                size,
                size,
                color,
            ));
        }
    }

    /// Upload the live instances at the start of `buffer`.
    pub fn write_buffer(&self, queue: &wgpu::Queue, buffer: &wgpu::Buffer) {
        let mut instances = Vec::new();
        self.push_instances(&mut instances);
        if instances.is_empty() {
            return;
        }
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&instances));
    }

    fn integrate(&mut self, origin: [f32; 2]) {
        self.spawn_debt += self.def.rate * FIXED_DT;
        while self.spawn_debt >= 1.0 {
            if !self.spawn(origin) {
                self.spawn_debt = 0.0;
                break;
            }
            self.spawn_debt -= 1.0;
        }
        for particle in &mut self.particles {
            if !particle.active {
                continue;
            }
            let wobble = (particle.age * 7.0 + particle.seed).sin() * self.def.turbulence;
            particle.vel[0] += wobble * FIXED_DT;
            particle.vel[0] += self.def.gravity[0] * FIXED_DT;
            particle.vel[1] += self.def.gravity[1] * FIXED_DT;
            particle.vel[2] += self.def.gravity[2] * FIXED_DT;
            particle.pos[0] += particle.vel[0] * FIXED_DT;
            particle.pos[1] += particle.vel[1] * FIXED_DT;
            particle.pos[2] += particle.vel[2] * FIXED_DT;
            particle.age += FIXED_DT;
            if particle.age >= particle.life
                || !particle.pos[0].is_finite()
                || !particle.pos[1].is_finite()
            {
                particle.active = false;
            }
        }
    }

    fn spawn(&mut self, origin: [f32; 2]) -> bool {
        let len = self.particles.len();
        if len == 0 {
            return false;
        }
        let start = self.cursor;
        for offset in 0..len {
            let index = (start + offset) % len;
            if self.particles[index].active {
                continue;
            }
            let angle = self.next_unit() * std::f32::consts::TAU;
            let seed = self.next_unit() * 10.0;
            let speed = self.def.speed;
            let life = self.def.lifetime.max(0.05);
            let particle = &mut self.particles[index];
            particle.active = true;
            particle.pos = [origin[0], origin[1], 0.0];
            particle.vel = [angle.cos() * speed, angle.sin() * speed, 0.0];
            particle.age = 0.0;
            particle.life = life;
            particle.seed = seed;
            self.cursor = (index + 1) % len;
            return true;
        }
        false
    }

    fn next_unit(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng >> 8) as f32 / 16_777_216.0
    }
}

fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn number(value: Option<&Value>) -> Option<f32> {
    let number = value?.as_f64()?;
    if number.is_finite() {
        Some(number as f32)
    } else {
        None
    }
}

fn vec3(value: Option<&Value>) -> Option<[f32; 3]> {
    let value = value?;
    if let Some(text) = value.as_str() {
        let mut parts = text.split_whitespace();
        let x = parts.next()?.parse().ok()?;
        let y = parts.next()?.parse().ok()?;
        let z = parts
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or(0.0);
        return Some([x, y, z]);
    }
    let array = value.as_array()?;
    let x = array.first().and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let y = array.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let z = array.get(2).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some([x, y, z])
}

fn color4(value: Option<&Value>) -> Option<[f32; 4]> {
    let value = value?;
    if let Some(text) = value.as_str() {
        let mut parts = text.split_whitespace().map(|part| part.parse::<f32>().ok());
        let r = parts.next().flatten()?;
        let g = parts.next().flatten().unwrap_or(r);
        let b = parts.next().flatten().unwrap_or(r);
        let a = parts.next().flatten().unwrap_or(1.0);
        return Some(normalize_color(r, g, b, a));
    }
    let array = value.as_array()?;
    let r = array.first().and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let g = array.get(1).and_then(Value::as_f64).unwrap_or(r as f64) as f32;
    let b = array.get(2).and_then(Value::as_f64).unwrap_or(r as f64) as f32;
    let a = array.get(3).and_then(Value::as_f64).unwrap_or(1.0) as f32;
    Some(normalize_color(r, g, b, a))
}

fn normalize_color(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    if r > 1.0 || g > 1.0 || b > 1.0 {
        [r / 255.0, g / 255.0, b / 255.0, a.clamp(0.0, 1.0)]
    } else {
        [r, g, b, a.clamp(0.0, 1.0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_never_passes_the_hard_cap() {
        let def = ParticleDef {
            capacity: 50_000,
            rate: 0.0,
            ..ParticleDef::default()
        };
        let emitter = Emitter::new(def);
        assert_eq!(emitter.capacity(), MAX_PARTICLES);
    }

    #[test]
    fn fixed_steps_spawn_a_bounded_count_and_stay_finite() {
        let json = br#"{
            "rate": 60,
            "lifetime": 2.0,
            "speed": 10,
            "gravity": [0, -20, 0],
            "turbulence": 4,
            "start_size": 8,
            "end_size": 1,
            "start_color": [1, 0, 0, 1],
            "end_color": [1, 0, 0, 0],
            "capacity": 200
        }"#;
        let def = ParticleDef::parse(json).expect("parse");
        let mut emitter = Emitter::new(def);
        emitter.step(1.0, [4.0, 5.0]);
        assert!(emitter.alive() > 0, "expected spawns");
        assert!(emitter.alive() <= 60 + 5, "alive {}", emitter.alive());
        let mut instances = Vec::new();
        emitter.push_instances(&mut instances);
        assert_eq!(instances.len(), emitter.alive());
        assert!(instances
            .iter()
            .all(|instance| instance.model_matrix[3][0].is_finite()));
    }

    #[test]
    fn wallpaper_engine_initializers_set_rate_and_gravity() {
        let json = br#"{
            "emitter": [{"name": "main", "rate": 12}],
            "initializer": [{"name": "lifetimerandom", "min": 1, "max": 3}],
            "operator": [{"name": "movement", "gravity": "0 -100 0"}]
        }"#;
        let def = ParticleDef::parse(json).expect("parse");
        assert_eq!(def.rate, 12.0);
        assert!((def.lifetime - 2.0).abs() < 0.01, "{}", def.lifetime);
        assert_eq!(def.gravity[1], -100.0);
    }
}
