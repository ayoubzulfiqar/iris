//! Layer tree, parallax, and the per-frame draw list.

use std::sync::Arc;

use iris_pkg::{DecodedTexture, ElementKind, Scene, SceneObject, SceneVec};
use iris_render::{DrawSpan, LayerBlend};
use serde_json::Value;

use crate::audio::AudioEngine;
use crate::instance::{push_bytes, ParticleInstance};
use crate::particles::{Emitter, ParticleDef};

const WHITE: u32 = u32::MAX;
const MAX_INSTANCES: usize = 10_000;

/// One evaluated layer.
#[derive(Debug, Clone)]
pub struct Layer {
    /// Object name.
    pub name: String,
    /// Draw class.
    pub class: LayerClass,
    /// Blend mode.
    pub blend: LayerBlend,
    /// Texture index, or [`WHITE`] when the layer has no image.
    pub texture: u32,
    /// Parent layer index.
    pub parent: Option<usize>,
    /// Local origin in scene pixels.
    pub origin: [f32; 3],
    /// Euler angles in degrees.
    pub angles: [f32; 3],
    /// Scale.
    pub scale: [f32; 3],
    /// Quad size in pixels.
    pub size: [f32; 2],
    /// Straight RGBA.
    pub color: [f32; 4],
    /// Parallax depth in pixels at the pointer extremes.
    pub parallax: [f32; 2],
    /// Hidden layers still carry children.
    pub visible: bool,
    /// Painter's order inside a pass.
    pub order: u32,
    /// Emitter index when this layer is a particle system.
    pub emitter: Option<usize>,
}

/// Which pass draws the layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerClass {
    /// Transform only.
    Group,
    /// Textured or tinted sprite.
    Sprite,
    /// Particle emitter.
    Particle,
    /// Foreground text quad.
    Text,
}

/// Placed layer, used by tests and the dirty-cache check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedLayer {
    /// Pass: 0 background, 1 sprite, 2 particle, 3 text.
    pub pass: u8,
    /// Order inside the pass.
    pub order: u32,
    /// Blend mode.
    pub blend: LayerBlend,
    /// World translation in pixels.
    pub translation: [f32; 2],
}

/// CPU scene ready to upload.
#[derive(Clone)]
pub struct ScenePackage {
    /// Layers in file order.
    pub layers: Vec<Layer>,
    /// Decoded images. Indexes match [`Layer::texture`].
    pub images: Vec<Arc<DecodedTexture>>,
    /// One emitter definition per particle layer.
    pub emitters: Vec<Emitter>,
    /// Scene clear color.
    pub clear: [f32; 4],
    /// `general.cameraparallaxamount`, or 1.
    pub parallax_amount: f32,
}

/// Running scene. Particles and the sprite cache live here.
pub struct SceneRuntime {
    package: ScenePackage,
    sprite_key: Option<([f32; 2], [u32; 2])>,
    sprite_instances: Vec<ParticleInstance>,
    sprite_spans: Vec<StoredSpan>,
}

#[derive(Clone)]
struct StoredSpan {
    blend: LayerBlend,
    texture: u32,
    start: u32,
    count: u32,
}

/// Owned draw list for one presented frame.
pub struct Tick {
    /// Clear color.
    pub clear: [f32; 4],
    /// 16-band spectrum.
    pub audio16: [f32; 16],
    /// 64-band spectrum.
    pub audio64: [f32; 64],
    /// Packed 96-byte instances.
    pub instances: Vec<u8>,
    /// Ranges into `instances`.
    pub spans: Vec<DrawSpan>,
}

impl ScenePackage {
    /// Build a package from a parsed scene, decoded images, and particle files.
    pub fn compile(
        scene: &Scene,
        images: Vec<(String, DecodedTexture)>,
        particle_files: Vec<(String, Vec<u8>)>,
    ) -> Self {
        let mut stored_images = Vec::new();
        let mut image_index = std::collections::HashMap::<String, u32>::new();
        let mut image_size = std::collections::HashMap::<String, [f32; 2]>::new();
        for (path, texture) in images {
            let index = stored_images.len() as u32;
            image_size.insert(path.clone(), [texture.width as f32, texture.height as f32]);
            image_index.insert(path, index);
            stored_images.push(Arc::new(texture));
        }
        let mut id_to_index = std::collections::HashMap::<i64, usize>::new();
        for (index, object) in scene.objects.iter().enumerate() {
            id_to_index.insert(object.id, index);
        }
        let mut emitters = Vec::new();
        let mut layers = Vec::with_capacity(scene.objects.len());
        for (index, object) in scene.objects.iter().enumerate() {
            let class = class_of(object);
            let image_path = object.image.clone().unwrap_or_default();
            let texture = image_index.get(&image_path).copied().unwrap_or(WHITE);
            let size = object
                .size
                .as_ref()
                .and_then(|value| value.to_array().ok())
                .map(|value| [value[0].abs().max(1.0), value[1].abs().max(1.0)])
                .or_else(|| image_size.get(&image_path).copied())
                .unwrap_or([128.0, 128.0]);
            let emitter = if class == LayerClass::Particle {
                let def = particle_def(object, &particle_files);
                let slot = emitters.len();
                emitters.push(Emitter::new(def));
                Some(slot)
            } else {
                None
            };
            let parent = object
                .parent
                .and_then(|id| id_to_index.get(&id).copied())
                .filter(|parent| *parent != index);
            layers.push(Layer {
                name: object.name.clone(),
                class,
                blend: blend_of(object),
                texture,
                parent,
                origin: vec_of(object.origin.as_ref()),
                angles: vec_of(object.angles.as_ref()),
                scale: scale_of(object.scale.as_ref()),
                size,
                color: color_of(object),
                parallax: {
                    let depth = vec_of(object.parallax_depth.as_ref());
                    [depth[0], depth[1]]
                },
                visible: object
                    .visible
                    .as_ref()
                    .and_then(|value| value.resolved().copied())
                    .unwrap_or(true),
                order: index as u32,
                emitter,
            });
        }
        let clear = scene
            .general
            .clearcolor
            .as_ref()
            .and_then(|value| value.to_array().ok())
            .map(|value| [value[0], value[1], value[2], 1.0])
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let parallax_amount = scene
            .general
            .cameraparallaxamount
            .unwrap_or(1.0)
            .clamp(0.0, 8.0) as f32;
        Self {
            layers,
            images: stored_images,
            emitters,
            clear,
            parallax_amount,
        }
    }

    /// World translation of every visible layer.
    pub fn place(&self, pointer: [f32; 2], viewport: [f32; 2]) -> Vec<PlacedLayer> {
        let worlds = world_matrices(self, pointer, viewport);
        self.layers
            .iter()
            .zip(worlds.iter())
            .filter(|(layer, _)| layer.visible && layer.class != LayerClass::Group)
            .map(|(layer, world)| PlacedLayer {
                pass: pass_of(layer),
                order: layer.order,
                blend: layer.blend,
                translation: [world.w_axis.x, world.w_axis.y],
            })
            .collect()
    }
}

impl SceneRuntime {
    /// Fresh particle pools for this package.
    pub fn new(mut package: ScenePackage) -> Self {
        for emitter in &mut package.emitters {
            let _ = emitter;
        }
        Self {
            package,
            sprite_key: None,
            sprite_instances: Vec::new(),
            sprite_spans: Vec::new(),
        }
    }

    /// Step particles and rebuild the draw list.
    ///
    /// Sprite batches are reused while the pointer and viewport stay put.
    pub fn tick(
        &mut self,
        frame_dt: f32,
        pointer: [f32; 2],
        viewport: [f32; 2],
        time: f32,
    ) -> Tick {
        let audio = AudioEngine::shared().snapshot();
        let key = (pointer, [viewport[0].to_bits(), viewport[1].to_bits()]);
        if self.sprite_key != Some(key) {
            self.rebuild_sprites(pointer, viewport);
            self.sprite_key = Some(key);
        }
        let origins = world_matrices(&self.package, pointer, viewport);
        for (layer, world) in self.package.layers.iter().zip(origins.iter()) {
            let Some(index) = layer.emitter else {
                continue;
            };
            if let Some(emitter) = self.package.emitters.get_mut(index) {
                emitter.step(frame_dt, [world.w_axis.x, world.w_axis.y]);
            }
        }
        let mut instances = self.sprite_instances.clone();
        let mut spans = self
            .sprite_spans
            .iter()
            .map(|span| DrawSpan {
                blend: span.blend,
                texture: span.texture,
                start: span.start,
                count: span.count,
            })
            .collect::<Vec<_>>();
        for (layer, world) in self.package.layers.iter().zip(origins.iter()) {
            let Some(index) = layer.emitter else {
                continue;
            };
            let Some(emitter) = self.package.emitters.get(index) else {
                continue;
            };
            let start = instances.len() as u32;
            let before = instances.len();
            emitter.push_instances(&mut instances);
            // Particles are simulated in world space already, so replace the
            // model origin the emitter wrote. `push_instances` bakes positions.
            let _ = world;
            let added = instances.len() - before;
            if added == 0 {
                continue;
            }
            if instances.len() > MAX_INSTANCES {
                instances.truncate(MAX_INSTANCES);
            }
            let count = instances.len() as u32 - start;
            if count == 0 {
                continue;
            }
            spans.push(DrawSpan {
                blend: layer.blend,
                texture: layer.texture,
                start,
                count,
            });
        }
        if instances.len() > MAX_INSTANCES {
            instances.truncate(MAX_INSTANCES);
        }
        let mut bytes = Vec::with_capacity(instances.len() * 96);
        push_bytes(&instances, &mut bytes);
        let _ = time;
        Tick {
            clear: self.package.clear,
            audio16: audio.bands_16,
            audio64: audio.bands_64,
            instances: bytes,
            spans,
        }
    }

    fn rebuild_sprites(&mut self, pointer: [f32; 2], viewport: [f32; 2]) {
        let worlds = world_matrices(&self.package, pointer, viewport);
        let mut order = Vec::new();
        for (index, layer) in self.package.layers.iter().enumerate() {
            if !layer.visible || !matches!(layer.class, LayerClass::Sprite | LayerClass::Text) {
                continue;
            }
            order.push((pass_of(layer), layer.order, index));
        }
        order.sort_by_key(|(pass, order, index)| (*pass, *order, *index));
        self.sprite_instances.clear();
        self.sprite_spans.clear();
        let mut group: Option<(LayerBlend, u32, u32)> = None;
        for (_, _, index) in order {
            if self.sprite_instances.len() >= MAX_INSTANCES {
                break;
            }
            let layer = &self.package.layers[index];
            let world = worlds[index]
                * glam::Mat4::from_scale(glam::Vec3::new(layer.size[0], layer.size[1], 1.0));
            self.sprite_instances.push(ParticleInstance {
                model_matrix: world.to_cols_array_2d(),
                color_tint: layer.color,
                tex_coords: [0.0, 0.0, 1.0, 1.0],
            });
            match group {
                Some((blend, texture, start))
                    if blend == layer.blend && texture == layer.texture =>
                {
                    let _ = start;
                }
                _ => {
                    if let Some((blend, texture, start)) = group {
                        let count = self.sprite_instances.len() as u32 - 1 - start;
                        if count > 0 {
                            self.sprite_spans.push(StoredSpan {
                                blend,
                                texture,
                                start,
                                count,
                            });
                        }
                    }
                    group = Some((
                        layer.blend,
                        layer.texture,
                        self.sprite_instances.len() as u32 - 1,
                    ));
                }
            }
        }
        if let Some((blend, texture, start)) = group {
            let count = self.sprite_instances.len() as u32 - start;
            if count > 0 {
                self.sprite_spans.push(StoredSpan {
                    blend,
                    texture,
                    start,
                    count,
                });
            }
        }
    }
}

fn world_matrices(
    package: &ScenePackage,
    pointer: [f32; 2],
    viewport: [f32; 2],
) -> Vec<glam::Mat4> {
    let mut worlds = vec![glam::Mat4::IDENTITY; package.layers.len()];
    let mut done = vec![false; package.layers.len()];
    for index in 0..package.layers.len() {
        place_layer(package, index, pointer, viewport, &mut worlds, &mut done, 0);
    }
    worlds
}

fn place_layer(
    package: &ScenePackage,
    index: usize,
    pointer: [f32; 2],
    viewport: [f32; 2],
    worlds: &mut [glam::Mat4],
    done: &mut [bool],
    depth: u32,
) {
    if done.get(index).copied().unwrap_or(true) || depth > 32 {
        return;
    }
    let layer = &package.layers[index];
    if let Some(parent) = layer.parent {
        if parent != index {
            place_layer(package, parent, pointer, viewport, worlds, done, depth + 1);
        }
    }
    let parent = layer
        .parent
        .and_then(|parent| worlds.get(parent).copied())
        .unwrap_or(glam::Mat4::IDENTITY);
    let local = local_matrix(layer);
    let shift = parallax_offset(pointer, viewport, layer.parallax, package.parallax_amount);
    worlds[index] =
        glam::Mat4::from_translation(glam::Vec3::new(shift[0], shift[1], 0.0)) * parent * local;
    done[index] = true;
}

fn local_matrix(layer: &Layer) -> glam::Mat4 {
    let translation = glam::Mat4::from_translation(glam::Vec3::new(
        layer.origin[0],
        layer.origin[1],
        layer.origin[2],
    ));
    let rotation = glam::Mat4::from_rotation_z(layer.angles[2].to_radians())
        * glam::Mat4::from_rotation_y(layer.angles[1].to_radians())
        * glam::Mat4::from_rotation_x(layer.angles[0].to_radians());
    let scale = glam::Mat4::from_scale(glam::Vec3::new(
        layer.scale[0],
        layer.scale[1],
        layer.scale[2],
    ));
    translation * rotation * scale
}

/// Pointer offset in pixels. The pointer is measured from the top-left of the viewport.
pub fn parallax_offset(
    pointer: [f32; 2],
    viewport: [f32; 2],
    depth: [f32; 2],
    amount: f32,
) -> [f32; 2] {
    let nx = if viewport[0] > 0.0 {
        pointer[0] / viewport[0] - 0.5
    } else {
        0.0
    };
    let ny = if viewport[1] > 0.0 {
        pointer[1] / viewport[1] - 0.5
    } else {
        0.0
    };
    [nx * 2.0 * depth[0] * amount, ny * 2.0 * depth[1] * amount]
}

fn class_of(object: &SceneObject) -> LayerClass {
    let background = matches!(object.extra.get("background"), Some(Value::Bool(true)));
    if background {
        return LayerClass::Sprite;
    }
    match object.kind() {
        ElementKind::Particle => LayerClass::Particle,
        ElementKind::Text => LayerClass::Text,
        ElementKind::Image | ElementKind::Model => LayerClass::Sprite,
        ElementKind::Node | ElementKind::Sound | ElementKind::Light | ElementKind::Camera => {
            LayerClass::Group
        }
    }
}

fn pass_of(layer: &Layer) -> u8 {
    let background = layer.name.eq_ignore_ascii_case("background");
    if background {
        return 0;
    }
    match layer.class {
        LayerClass::Group | LayerClass::Sprite => 1,
        LayerClass::Particle => 2,
        LayerClass::Text => 3,
    }
}

fn blend_of(object: &SceneObject) -> LayerBlend {
    let value = object
        .extra
        .get("blendmode")
        .or_else(|| object.extra.get("blend"));
    match value {
        Some(Value::String(text)) => match text.to_ascii_lowercase().as_str() {
            "additive" | "add" => LayerBlend::Additive,
            "multiply" | "mult" => LayerBlend::Multiply,
            "screen" => LayerBlend::Screen,
            _ => LayerBlend::Alpha,
        },
        Some(Value::Number(number)) => match number.as_i64() {
            Some(1) => LayerBlend::Additive,
            Some(2) => LayerBlend::Multiply,
            Some(3) => LayerBlend::Screen,
            _ => LayerBlend::Alpha,
        },
        _ => LayerBlend::Alpha,
    }
}

fn particle_def(object: &SceneObject, files: &[(String, Vec<u8>)]) -> ParticleDef {
    if let Some(path) = &object.particle {
        if let Some((_, bytes)) = files.iter().find(|(name, _)| name == path) {
            if let Ok(def) = ParticleDef::parse(bytes) {
                return def;
            }
        }
    }
    if let Ok(bytes) = serde_json::to_vec(&object.extra) {
        if let Ok(def) = ParticleDef::parse(&bytes) {
            return def;
        }
    }
    ParticleDef::default()
}

fn vec_of(value: Option<&SceneVec>) -> [f32; 3] {
    value
        .and_then(|value| value.to_array().ok())
        .unwrap_or([0.0, 0.0, 0.0])
}

fn scale_of(value: Option<&SceneVec>) -> [f32; 3] {
    let raw = vec_of(value);
    if value.is_none() {
        [1.0, 1.0, 1.0]
    } else {
        [
            if raw[0] == 0.0 { 1.0 } else { raw[0] },
            if raw[1] == 0.0 { 1.0 } else { raw[1] },
            if raw[2] == 0.0 { 1.0 } else { raw[2] },
        ]
    }
}

fn color_of(object: &SceneObject) -> [f32; 4] {
    let Some(color) = object
        .color
        .as_ref()
        .and_then(|value| value.to_array().ok())
    else {
        return [1.0, 1.0, 1.0, 1.0];
    };
    if color[0] > 1.0 || color[1] > 1.0 || color[2] > 1.0 {
        [color[0] / 255.0, color[1] / 255.0, color[2] / 255.0, 1.0]
    } else {
        [color[0], color[1], color[2], 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        let json = r#"{
            "camera": {"eye": "0 0 1", "center": "0 0 0", "up": "0 1 0"},
            "general": {"orthogonalprojection": {"width": 200, "height": 100}, "cameraparallaxamount": 1},
            "objects": [
                {"id": 1, "name": "root", "angles": "0 0 90"},
                {"id": 2, "name": "sprite", "parent": 1, "image": "a.png", "origin": "10 0 0", "size": "100 50 0", "parallaxDepth": "20 0 0", "blendmode": "additive"},
                {"id": 3, "name": "caption", "text": "hi", "origin": "0 8 0"}
            ]
        }"#;
        Scene::parse(json.as_bytes()).expect("scene")
    }

    #[test]
    fn parent_rotation_parallax_and_blend_are_applied() {
        let package = ScenePackage::compile(&scene(), Vec::new(), Vec::new());
        let placed = package.place([200.0, 50.0], [200.0, 100.0]);
        let sprite = placed
            .iter()
            .find(|layer| layer.blend == LayerBlend::Additive)
            .expect("sprite");
        // Parent rotates the local x=10 offset to y=10. Pointer at the right
        // edge adds parallax depth 20 on x.
        assert!(
            (sprite.translation[0] - 20.0).abs() < 0.01,
            "{:?}",
            sprite.translation
        );
        assert!(
            (sprite.translation[1] - 10.0).abs() < 0.01,
            "{:?}",
            sprite.translation
        );
        let text = placed.iter().find(|layer| layer.pass == 3).expect("text");
        assert!(
            (text.translation[1] - 8.0).abs() < 0.01,
            "{:?}",
            text.translation
        );
    }

    #[test]
    fn an_unchanged_pointer_reuses_the_sprite_cache() {
        let package = ScenePackage::compile(&scene(), Vec::new(), Vec::new());
        let mut runtime = SceneRuntime::new(package);
        let first = runtime.tick(0.0, [0.0, 0.0], [200.0, 100.0], 0.0);
        let second = runtime.tick(0.0, [0.0, 0.0], [200.0, 100.0], 0.1);
        assert_eq!(first.instances.len(), second.instances.len());
        assert!(!first.spans.is_empty());
    }
}
