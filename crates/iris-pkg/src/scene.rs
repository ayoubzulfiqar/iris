//! Wallpaper Engine `scene.json` and `project.json`.
//!
//! Vectors accept the forms the editor writes: a finite number (broadcast to
//! three components), a whitespace-separated string (`"x y"` or `"x y z"`),
//! a one- to three-element array, or a user binding `{"value": ...}`. Bindings
//! nest at most four levels. Unknown keys stay on [`Scene::extra`] and the
//! matching `extra` maps so a later pass can round-trip them.
//!
//! [`Scene::from_package`] reads `scene.json` at the archive root when it
//! exists, and otherwise the first `*/scene.json` entry.

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::Error;
use crate::pkg::Package;

const MAX_BIND_DEPTH: u32 = 4;

/// A parsed `scene.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scene {
    /// Default camera.
    pub camera: Camera,
    /// Scene-wide render settings.
    pub general: General,
    /// Objects in archive order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<SceneObject>,
    /// Scene format version. Missing versions become `0`.
    pub version: i64,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// Look-at camera. Missing axes become [`SceneVec::ZERO`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Camera {
    /// Look-at target.
    pub center: SceneVec,
    /// Camera position.
    pub eye: SceneVec,
    /// Up direction.
    pub up: SceneVec,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// Scene-wide settings. Absent keys stay `None` and are left out on write.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct General {
    /// Orthographic scene size, when the scene is 2D.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orthogonalprojection: Option<OrthoProjection>,
    /// Ambient light color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambientcolor: Option<SceneVec>,
    /// Clear color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearcolor: Option<SceneVec>,
    /// Skylight color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skylightcolor: Option<SceneVec>,
    /// Whether the color buffer is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearenabled: Option<bool>,
    /// Bloom toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bloom: Option<bool>,
    /// HDR toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<bool>,
    /// Camera fade toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camerafade: Option<bool>,
    /// Editor preview camera toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camerapreview: Option<bool>,
    /// Bloom strength.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bloomstrength: Option<f64>,
    /// Bloom threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bloomthreshold: Option<f64>,
    /// Near clip plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearz: Option<f64>,
    /// Far clip plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farz: Option<f64>,
    /// Perspective field of view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f64>,
    /// Camera zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Parallax amount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cameraparallaxamount: Option<f64>,
    /// Parallax delay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cameraparallaxdelay: Option<f64>,
    /// Keys this schema does not model, including bloom HDR and wind blocks.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// `general.orthogonalprojection` width and height.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrthoProjection {
    /// Scene width in pixels.
    pub width: f64,
    /// Scene height in pixels.
    pub height: f64,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// One element of `objects`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SceneObject {
    /// Object id. Missing ids become `0`.
    pub id: i64,
    /// Display name. Missing names become an empty string.
    pub name: String,
    /// Image or material path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Model reference. A string path and an object are both kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Value>,
    /// Particle preset path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle: Option<String>,
    /// Sound paths. A single string and an array both become a list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sound: Vec<String>,
    /// Text payload. A string and a bound object are both kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Value>,
    /// Font path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Light preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<String>,
    /// Camera preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Translation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SceneVec>,
    /// Euler angles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angles: Option<SceneVec>,
    /// Scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<SceneVec>,
    /// Size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<SceneVec>,
    /// Tint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<SceneVec>,
    /// Parallax offset. JSON key `parallaxDepth`.
    #[serde(
        default,
        rename = "parallaxDepth",
        skip_serializing_if = "Option::is_none"
    )]
    pub parallax_depth: Option<SceneVec>,
    /// Direct bool or a bound property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<UserValue<bool>>,
    /// Shader effects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<SceneEffect>,
    /// Parent object id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<i64>,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// A shader effect on a [`SceneObject`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SceneEffect {
    /// Effect id. Missing ids become `0`.
    pub id: i64,
    /// Effect name. Missing names become an empty string.
    pub name: String,
    /// Effect JSON path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Direct bool or a bound property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<UserValue<bool>>,
    /// Render passes, preserved as JSON.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passes: Vec<Value>,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// Which populated field classifies a [`SceneObject`].
///
/// Wallpaper Engine stores every element in one array. Image wins over model,
/// then particle, sound, text or font, light, and camera. Anything else is a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    /// `image` is set.
    Image,
    /// `model` is set.
    Model,
    /// `particle` is set.
    Particle,
    /// `sound` is non-empty.
    Sound,
    /// `text` or `font` is set.
    Text,
    /// `light` is set.
    Light,
    /// `camera` is set.
    Camera,
    /// Transform-only element.
    Node,
}

/// A literal value or a user-property binding (`{"value": ..., ...}`).
#[derive(Debug, Clone, PartialEq)]
pub enum UserValue<T> {
    /// A bare JSON value.
    Literal(T),
    /// An object binding. `value` is absent when the object has no `value` key.
    Bound {
        /// Resolved fallback stored on the binding.
        value: Option<T>,
        /// Remaining keys, such as `user`.
        binding: Map<String, Value>,
    },
}

/// Three finite components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneVec {
    components: [f32; 3],
}

/// A parsed `project.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    /// Wallpaper title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Wallpaper kind. JSON key `type`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub wallpaper_type: Option<String>,
    /// Main scene file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Preview image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Workshop tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Workshop item id. A number or a string.
    #[serde(
        default,
        rename = "workshopid",
        skip_serializing_if = "Option::is_none"
    )]
    pub workshopid: Option<WorkshopId>,
    /// Project version. A number or a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Value>,
    /// Content rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contentrating: Option<String>,
    /// Editor general block, preserved as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<Value>,
    /// Keys this schema does not model.
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

/// `project.json` `workshopid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkshopId {
    /// Unsigned integer id.
    Number(u64),
    /// Id written as a string.
    Text(String),
}

impl Scene {
    /// Parse a `scene.json` document. A leading UTF-8 BOM is ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_value(decode_json(bytes, "scene.json")?)
    }

    /// Parse a JSON value that is already decoded.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        let mut map = as_object(value, "scene.json", "scene")?;
        let mut camera = as_object(
            required(&mut map, "camera", "scene.json")?,
            "scene.json",
            "camera",
        )?;
        let general = parse_general(required(&mut map, "general", "scene.json")?)?;
        let objects = parse_objects(map.remove("objects"))?;
        let version = take_i64(&mut map, "version", "scene.json")?.unwrap_or(0);
        Ok(Self {
            camera: Camera {
                center: take_vec(&mut camera, "center")?.unwrap_or(SceneVec::ZERO),
                eye: take_vec(&mut camera, "eye")?.unwrap_or(SceneVec::ZERO),
                up: take_vec(&mut camera, "up")?.unwrap_or(SceneVec::ZERO),
                extra: camera,
            },
            general,
            objects,
            version,
            extra: map,
        })
    }

    /// Read `scene.json`, or `*/scene.json` when the root name is absent.
    ///
    /// Returns `Ok(None)` when the package has neither entry.
    pub fn from_package(package: &Package) -> Result<Option<Self>, Error> {
        match find_document(package, "scene.json") {
            Some(bytes) => Ok(Some(Self::parse(bytes)?)),
            None => Ok(None),
        }
    }
}

impl SceneObject {
    /// Classify this element from the fields the editor populated.
    #[must_use]
    pub fn kind(&self) -> ElementKind {
        if self.image.is_some() {
            ElementKind::Image
        } else if self.model.is_some() {
            ElementKind::Model
        } else if self.particle.is_some() {
            ElementKind::Particle
        } else if !self.sound.is_empty() {
            ElementKind::Sound
        } else if self.text.is_some() || self.font.is_some() {
            ElementKind::Text
        } else if self.light.is_some() {
            ElementKind::Light
        } else if self.camera.is_some() {
            ElementKind::Camera
        } else {
            ElementKind::Node
        }
    }
}

impl Project {
    /// Parse a `project.json` document. A leading UTF-8 BOM is ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_value(decode_json(bytes, "project.json")?)
    }

    /// Parse a JSON value that is already decoded.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        let mut map = as_object(value, "project.json", "project")?;
        let workshopid = match map.remove("workshopid") {
            None | Some(Value::Null) => None,
            Some(value) => Some(parse_workshop_id(&value)?),
        };
        Ok(Self {
            title: take_string(&mut map, "title", "project.json")?,
            wallpaper_type: take_string(&mut map, "type", "project.json")?,
            file: take_string(&mut map, "file", "project.json")?,
            preview: take_string(&mut map, "preview", "project.json")?,
            description: take_string(&mut map, "description", "project.json")?,
            tags: take_tags(&mut map)?,
            workshopid,
            version: take_version(&mut map)?,
            contentrating: take_string(&mut map, "contentrating", "project.json")?,
            general: match map.remove("general") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value),
            },
            extra: map,
        })
    }

    /// Read `project.json`, or `*/project.json` when the root name is absent.
    ///
    /// Returns `Ok(None)` when the package has neither entry.
    pub fn from_package(package: &Package) -> Result<Option<Self>, Error> {
        match find_document(package, "project.json") {
            Some(bytes) => Ok(Some(Self::parse(bytes)?)),
            None => Ok(None),
        }
    }
}

impl SceneVec {
    /// The zero vector.
    pub const ZERO: Self = Self {
        components: [0.0, 0.0, 0.0],
    };

    /// Parse one vector value.
    ///
    /// `context` is the field name reported in [`Error::BadVector`].
    pub fn from_value(value: &Value, context: &'static str) -> Result<Self, Error> {
        Ok(Self {
            components: read_components(value, context, 0)?,
        })
    }

    /// Copy the three components.
    pub fn to_array(&self) -> Result<[f32; 3], Error> {
        if self
            .components
            .iter()
            .all(|component| component.is_finite())
        {
            Ok(self.components)
        } else {
            Err(Error::BadVector {
                context: "scene vector",
                detail: "component is not finite".to_owned(),
            })
        }
    }
}

impl<'de> Deserialize<'de> for Scene {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for Project {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(de::Error::custom)
    }
}

impl Serialize for SceneVec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.components.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SceneVec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(&value, "vector").map_err(de::Error::custom)
    }
}

impl<T> UserValue<T> {
    /// The literal, or the binding's `value` when it has one.
    #[must_use]
    pub fn resolved(&self) -> Option<&T> {
        match self {
            Self::Literal(value) => Some(value),
            Self::Bound { value, .. } => value.as_ref(),
        }
    }
}

impl<T: Serialize> Serialize for UserValue<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Literal(value) => value.serialize(serializer),
            Self::Bound { value, binding } => {
                let mut object = binding.clone();
                if let Some(inner) = value {
                    let encoded = serde_json::to_value(inner).map_err(serde::ser::Error::custom)?;
                    object.insert("value".to_owned(), encoded);
                }
                object.serialize(serializer)
            }
        }
    }
}

impl<'de, T> Deserialize<'de> for UserValue<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        read_user_value(value).map_err(de::Error::custom)
    }
}

impl Serialize for WorkshopId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Number(id) => id.serialize(serializer),
            Self::Text(text) => text.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for WorkshopId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        parse_workshop_id(&value).map_err(de::Error::custom)
    }
}

fn parse_general(value: Value) -> Result<General, Error> {
    let mut map = as_object(value, "scene.json", "general")?;
    Ok(General {
        orthogonalprojection: take_ortho(&mut map)?,
        ambientcolor: take_vec(&mut map, "ambientcolor")?,
        clearcolor: take_vec(&mut map, "clearcolor")?,
        skylightcolor: take_vec(&mut map, "skylightcolor")?,
        clearenabled: take_bool(&mut map, "clearenabled", "scene.json")?,
        bloom: take_bool(&mut map, "bloom", "scene.json")?,
        hdr: take_bool(&mut map, "hdr", "scene.json")?,
        camerafade: take_bool(&mut map, "camerafade", "scene.json")?,
        camerapreview: take_bool(&mut map, "camerapreview", "scene.json")?,
        bloomstrength: take_f64(&mut map, "bloomstrength", "scene.json")?,
        bloomthreshold: take_f64(&mut map, "bloomthreshold", "scene.json")?,
        nearz: take_f64(&mut map, "nearz", "scene.json")?,
        farz: take_f64(&mut map, "farz", "scene.json")?,
        fov: take_f64(&mut map, "fov", "scene.json")?,
        zoom: take_f64(&mut map, "zoom", "scene.json")?,
        cameraparallaxamount: take_f64(&mut map, "cameraparallaxamount", "scene.json")?,
        cameraparallaxdelay: take_f64(&mut map, "cameraparallaxdelay", "scene.json")?,
        extra: map,
    })
}

fn parse_objects(value: Option<Value>) -> Result<Vec<SceneObject>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.into_iter().map(parse_object).collect(),
        Some(_) => Err(json_err("scene.json", "objects must be an array")),
    }
}

fn parse_object(value: Value) -> Result<SceneObject, Error> {
    let mut map = as_object(value, "scene.json", "object")?;
    Ok(SceneObject {
        id: take_i64(&mut map, "id", "scene.json")?.unwrap_or(0),
        name: take_string(&mut map, "name", "scene.json")?.unwrap_or_default(),
        image: take_string(&mut map, "image", "scene.json")?,
        model: take_present(&mut map, "model"),
        particle: take_string(&mut map, "particle", "scene.json")?,
        sound: take_sound(&mut map)?,
        text: take_present(&mut map, "text"),
        font: take_string(&mut map, "font", "scene.json")?,
        light: take_string(&mut map, "light", "scene.json")?,
        camera: take_string(&mut map, "camera", "scene.json")?,
        origin: take_vec(&mut map, "origin")?,
        angles: take_vec(&mut map, "angles")?,
        scale: take_vec(&mut map, "scale")?,
        size: take_vec(&mut map, "size")?,
        color: take_vec(&mut map, "color")?,
        parallax_depth: take_vec(&mut map, "parallaxDepth")?,
        visible: take_user(&mut map, "visible")?,
        effects: parse_effects(map.remove("effects"))?,
        parent: take_i64(&mut map, "parent", "scene.json")?,
        extra: map,
    })
}

fn parse_effects(value: Option<Value>) -> Result<Vec<SceneEffect>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.into_iter().map(parse_effect).collect(),
        Some(_) => Err(json_err("scene.json", "effects must be an array")),
    }
}

fn parse_effect(value: Value) -> Result<SceneEffect, Error> {
    let mut map = as_object(value, "scene.json", "effect")?;
    let passes = match map.remove("passes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(json_err("scene.json", "passes must be an array")),
    };
    Ok(SceneEffect {
        id: take_i64(&mut map, "id", "scene.json")?.unwrap_or(0),
        name: take_string(&mut map, "name", "scene.json")?.unwrap_or_default(),
        file: take_string(&mut map, "file", "scene.json")?,
        visible: take_user(&mut map, "visible")?,
        passes,
        extra: map,
    })
}

fn take_ortho(map: &mut Map<String, Value>) -> Result<Option<OrthoProjection>, Error> {
    match map.remove("orthogonalprojection") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(mut inner)) => {
            let width = require_f64(&mut inner, "width")?;
            let height = require_f64(&mut inner, "height")?;
            if width < 0.0 || height < 0.0 {
                return Err(json_err(
                    "scene.json",
                    "orthogonalprojection width and height must be >= 0",
                ));
            }
            Ok(Some(OrthoProjection {
                width,
                height,
                extra: inner,
            }))
        }
        Some(_) => Err(json_err(
            "scene.json",
            "orthogonalprojection must be an object",
        )),
    }
}

fn require_f64(map: &mut Map<String, Value>, key: &'static str) -> Result<f64, Error> {
    take_f64(map, key, "scene.json")?.ok_or_else(|| {
        json_err(
            "scene.json",
            format!("orthogonalprojection is missing {key}"),
        )
    })
}

fn take_vec(map: &mut Map<String, Value>, key: &'static str) -> Result<Option<SceneVec>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(SceneVec::from_value(&value, key)?)),
    }
}

fn take_user(
    map: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Option<UserValue<bool>>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(read_user_value(value)?)),
    }
}

fn take_present(map: &mut Map<String, Value>, key: &str) -> Option<Value> {
    match map.remove(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value),
    }
}

fn take_sound(map: &mut Map<String, Value>) -> Result<Vec<String>, Error> {
    match map.remove("sound") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(path)) => Ok(vec![path]),
        Some(Value::Array(items)) => {
            let mut paths = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(path) => paths.push(path),
                    _ => {
                        return Err(json_err("scene.json", "sound entries must be strings"));
                    }
                }
            }
            Ok(paths)
        }
        Some(_) => Err(json_err(
            "scene.json",
            "sound must be a string or an array of strings",
        )),
    }
}

fn take_tags(map: &mut Map<String, Value>) -> Result<Vec<String>, Error> {
    match map.remove("tags") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut tags = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(tag) => tags.push(tag),
                    _ => return Err(json_err("project.json", "tags must be strings")),
                }
            }
            Ok(tags)
        }
        Some(_) => Err(json_err("project.json", "tags must be an array of strings")),
    }
}

fn take_version(map: &mut Map<String, Value>) -> Result<Option<Value>, Error> {
    match map.remove("version") {
        None | Some(Value::Null) => Ok(None),
        Some(value @ (Value::Number(_) | Value::String(_))) => Ok(Some(value)),
        Some(_) => Err(json_err(
            "project.json",
            "version must be a string or a number",
        )),
    }
}

fn take_string(
    map: &mut Map<String, Value>,
    key: &'static str,
    document: &'static str,
) -> Result<Option<String>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(json_err(document, format!("{key} must be a string"))),
    }
}

fn take_bool(
    map: &mut Map<String, Value>,
    key: &'static str,
    document: &'static str,
) -> Result<Option<bool>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(flag)),
        Some(_) => Err(json_err(document, format!("{key} must be a bool"))),
    }
}

fn take_i64(
    map: &mut Map<String, Value>,
    key: &'static str,
    document: &'static str,
) -> Result<Option<i64>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| json_err(document, format!("{key} is not an integer"))),
        Some(_) => Err(json_err(document, format!("{key} must be a number"))),
    }
}

fn take_f64(
    map: &mut Map<String, Value>,
    key: &'static str,
    document: &'static str,
) -> Result<Option<f64>, Error> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
                return Err(json_err(document, format!("{key} is not a finite number")));
            };
            Ok(Some(value))
        }
        Some(_) => Err(json_err(document, format!("{key} must be a number"))),
    }
}

fn read_components(value: &Value, context: &'static str, depth: u32) -> Result<[f32; 3], Error> {
    match value {
        Value::Number(number) => {
            let scalar = finite_f64(number, context)?;
            let component = narrow(scalar, context)?;
            Ok([component, component, component])
        }
        Value::String(text) => components_from_text(text, context),
        Value::Array(items) => components_from_array(items, context),
        Value::Object(map) => {
            if depth >= MAX_BIND_DEPTH {
                return Err(Error::BadVector {
                    context,
                    detail: format!("value binding is nested more than {MAX_BIND_DEPTH} levels"),
                });
            }
            let Some(inner) = map.get("value") else {
                return Err(Error::BadVector {
                    context,
                    detail: "object vector has no value field".to_owned(),
                });
            };
            read_components(inner, context, depth.saturating_add(1))
        }
        Value::Bool(_) => Err(Error::BadVector {
            context,
            detail: "bool is not a vector".to_owned(),
        }),
        Value::Null => Err(Error::BadVector {
            context,
            detail: "null is not a vector".to_owned(),
        }),
    }
}

fn components_from_text(text: &str, context: &'static str) -> Result<[f32; 3], Error> {
    let mut parts = [0.0_f32; 3];
    let mut count = 0_usize;
    for token in text.split_whitespace() {
        if count == 3 {
            return Err(Error::BadVector {
                context,
                detail: format!("`{text}` has more than 3 components"),
            });
        }
        let parsed = token.parse::<f64>().map_err(|_| Error::BadVector {
            context,
            detail: format!("`{token}` is not a number"),
        })?;
        let slot = parts
            .get_mut(count)
            .ok_or(Error::Invariant("vector slot"))?;
        *slot = narrow(parsed, context)?;
        count = count.saturating_add(1);
    }
    pack_components(parts, count, context, text)
}

fn components_from_array(items: &[Value], context: &'static str) -> Result<[f32; 3], Error> {
    if items.len() > 3 {
        return Err(Error::BadVector {
            context,
            detail: format!("array has {} components", items.len()),
        });
    }
    let mut parts = [0.0_f32; 3];
    for (index, item) in items.iter().enumerate() {
        let Value::Number(number) = item else {
            return Err(Error::BadVector {
                context,
                detail: "array component is not a number".to_owned(),
            });
        };
        let slot = parts
            .get_mut(index)
            .ok_or(Error::Invariant("vector slot"))?;
        *slot = narrow(finite_f64(number, context)?, context)?;
    }
    pack_components(parts, items.len(), context, "[]")
}

fn pack_components(
    parts: [f32; 3],
    count: usize,
    context: &'static str,
    original: &str,
) -> Result<[f32; 3], Error> {
    match count {
        1 => Ok([parts[0], parts[0], parts[0]]),
        2 => Ok([parts[0], parts[1], 0.0]),
        3 => Ok(parts),
        _ => Err(Error::BadVector {
            context,
            detail: format!("`{original}` is empty"),
        }),
    }
}

fn finite_f64(number: &serde_json::Number, context: &'static str) -> Result<f64, Error> {
    match number.as_f64() {
        Some(value) if value.is_finite() => Ok(value),
        _ => Err(Error::BadVector {
            context,
            detail: format!("{number} is outside the finite range"),
        }),
    }
}

fn narrow(value: f64, context: &'static str) -> Result<f32, Error> {
    let narrowed = value as f32;
    if value.is_finite() && narrowed.is_finite() {
        Ok(narrowed)
    } else {
        Err(Error::BadVector {
            context,
            detail: format!("{value} is not a finite component"),
        })
    }
}

fn read_user_value<T: DeserializeOwned>(value: Value) -> Result<UserValue<T>, Error> {
    match value {
        Value::Object(mut map) => {
            let value = match map.remove("value") {
                None => None,
                Some(inner) => {
                    Some(serde_json::from_value(inner).map_err(|source| Error::Json {
                        context: "scene.json",
                        source,
                    })?)
                }
            };
            Ok(UserValue::Bound {
                value,
                binding: map,
            })
        }
        other => {
            let literal = serde_json::from_value(other).map_err(|source| Error::Json {
                context: "scene.json",
                source,
            })?;
            Ok(UserValue::Literal(literal))
        }
    }
}

fn parse_workshop_id(value: &Value) -> Result<WorkshopId, Error> {
    match value {
        Value::String(text) => Ok(WorkshopId::Text(text.clone())),
        Value::Number(number) => match number.as_u64() {
            Some(id) => Ok(WorkshopId::Number(id)),
            None => Err(json_err(
                "project.json",
                format!("workshopid {number} is not an unsigned integer"),
            )),
        },
        _ => Err(json_err(
            "project.json",
            "workshopid must be a string or an unsigned integer",
        )),
    }
}

fn required(
    map: &mut Map<String, Value>,
    key: &str,
    document: &'static str,
) -> Result<Value, Error> {
    map.remove(key)
        .ok_or_else(|| json_err(document, format!("missing {key}")))
}

fn as_object(
    value: Value,
    document: &'static str,
    what: &str,
) -> Result<Map<String, Value>, Error> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(json_err(document, format!("{what} must be a JSON object"))),
    }
}

fn json_err(document: &'static str, detail: impl Into<String>) -> Error {
    Error::Json {
        context: document,
        source: <serde_json::Error as de::Error>::custom(detail.into()),
    }
}

fn decode_json(bytes: &[u8], document: &'static str) -> Result<Value, Error> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|source| Error::Json {
        context: document,
        source,
    })
}

fn find_document<'a>(package: &'a Package, file_name: &str) -> Option<&'a [u8]> {
    if let Some(bytes) = package.get(file_name) {
        return Some(bytes);
    }
    let suffix = format!("/{file_name}");
    package.entries().iter().find_map(|entry| {
        entry
            .path
            .ends_with(&suffix)
            .then_some(entry.bytes.as_slice())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{must, push_sized, push_u32};

    const SAMPLE: &str = r#"{
        "camera": {"center": "0 0 0", "eye": "0 0 5", "up": "0 1 0"},
        "general": {
            "orthogonalprojection": {"width": 1920, "height": 1080},
            "clearcolor": "0 0 0",
            "bloomhdrstrength": 0.25
        },
        "objects": [{
            "id": 7,
            "name": "layer",
            "image": "materials/bg.json",
            "model": "models/unused.mdl",
            "parallaxDepth": "1 0",
            "visible": true,
            "copybackground": true,
            "origin": 2,
            "size": {"value": "9 8 7"}
        }],
        "version": 1
    }"#;

    #[test]
    fn scene_vectors_visibility_and_unknown_keys_roundtrip() {
        let scene = must(Scene::parse(SAMPLE.as_bytes()));
        let object = scene.objects.first().expect("object");
        assert_eq!(
            must(object.parallax_depth.expect("depth").to_array()),
            [1.0, 0.0, 0.0]
        );
        assert_eq!(
            object.visible.as_ref().and_then(UserValue::resolved),
            Some(&true)
        );
        assert_eq!(object.kind(), ElementKind::Image);
        assert_eq!(
            must(object.origin.expect("origin").to_array()),
            [2.0, 2.0, 2.0]
        );
        assert_eq!(must(object.size.expect("size").to_array()), [9.0, 8.0, 7.0]);
        assert_eq!(object.extra.get("copybackground"), Some(&Value::Bool(true)));
        assert_eq!(
            scene.general.extra.get("bloomhdrstrength"),
            Some(&Value::from(0.25))
        );
        assert_eq!(
            scene
                .general
                .orthogonalprojection
                .as_ref()
                .map(|ortho| (ortho.width, ortho.height)),
            Some((1920.0, 1080.0))
        );

        let encoded = must(serde_json::to_vec(&scene));
        let again = must(Scene::parse(&encoded));
        assert_eq!(again, scene);
        assert_eq!(
            again.objects[0].extra.get("copybackground"),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn a_utf8_bom_is_ignored() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SAMPLE.as_bytes());
        let scene = must(Scene::parse(&bytes));
        assert_eq!(scene.version, 1);
    }

    #[test]
    fn kind_follows_image_model_particle_sound_text_light_camera() {
        assert_eq!(
            kind_of(r#"{"image":"a.json","sound":"a.mp3"}"#),
            ElementKind::Image
        );
        assert_eq!(kind_of(r#"{"model":"a.mdl"}"#), ElementKind::Model);
        assert_eq!(
            kind_of(r#"{"particle":"a.json","sound":["a.mp3"]}"#),
            ElementKind::Particle
        );
        assert_eq!(kind_of(r#"{"sound":"a.mp3"}"#), ElementKind::Sound);
        assert_eq!(
            kind_of(r#"{"sound":["a.mp3","b.mp3"]}"#),
            ElementKind::Sound
        );
        assert_eq!(kind_of(r#"{"text":"hi"}"#), ElementKind::Text);
        assert_eq!(kind_of(r#"{"font":"font.ttf"}"#), ElementKind::Text);
        assert_eq!(kind_of(r#"{"light":"point"}"#), ElementKind::Light);
        assert_eq!(kind_of(r#"{"camera":"main"}"#), ElementKind::Camera);
        assert_eq!(kind_of(r#"{"name":"group"}"#), ElementKind::Node);
    }

    #[test]
    fn bound_visibility_keeps_its_user_key() {
        let scene = scene_with_object(r#"{"visible":{"user":"hide","value":false}}"#);
        let visible = scene.objects[0].visible.as_ref().expect("visible");
        assert_eq!(visible.resolved(), Some(&false));
        match visible {
            UserValue::Bound { binding, .. } => {
                assert_eq!(binding.get("user").and_then(Value::as_str), Some("hide"));
            }
            UserValue::Literal(_) => panic!("expected a binding"),
        }
    }

    #[test]
    fn four_binding_levels_broadcast_and_a_fifth_is_rejected() {
        let scene = scene_with_object(&format!(r#"{{"origin":{}}}"#, nested_value(4)));
        assert_eq!(
            must(scene.objects[0].origin.expect("origin").to_array()),
            [1.0, 1.0, 1.0]
        );
        let broken = format!(
            r#"{{"camera":{{}},"general":{{}},"objects":[{{"origin":{}}}]}}"#,
            nested_value(5)
        );
        assert!(matches!(
            Scene::parse(broken.as_bytes()),
            Err(Error::BadVector {
                context: "origin",
                ..
            })
        ));
    }

    #[test]
    fn a_bad_vector_is_not_a_json_syntax_error() {
        let broken = r#"{"camera":{},"general":{},"objects":[{"origin":"1 2 3 4"}]}"#;
        assert!(matches!(
            Scene::parse(broken.as_bytes()),
            Err(Error::BadVector {
                context: "origin",
                ..
            })
        ));
    }

    #[test]
    fn invalid_json_and_a_missing_camera_fail() {
        assert!(matches!(
            Scene::parse(b"{"),
            Err(Error::Json {
                context: "scene.json",
                ..
            })
        ));
        assert!(matches!(
            Scene::parse(br#"{"general":{}}"#),
            Err(Error::Json {
                context: "scene.json",
                ..
            })
        ));
    }

    #[test]
    fn from_package_prefers_the_root_scene_and_misses_nothing() {
        let nested = package(&[(
            "scenes/scene.json",
            r#"{"camera":{},"general":{},"version":2}"#,
        )]);
        let scene = must(Scene::from_package(&nested)).expect("nested scene");
        assert_eq!(scene.version, 2);

        let both = package(&[
            (
                "scenes/scene.json",
                r#"{"camera":{},"general":{},"version":2}"#,
            ),
            ("scene.json", r#"{"camera":{},"general":{},"version":1}"#),
        ]);
        let scene = must(Scene::from_package(&both)).expect("root scene");
        assert_eq!(scene.version, 1);

        let empty = package(&[]);
        assert!(must(Scene::from_package(&empty)).is_none());

        let corrupt = package(&[("scene.json", "{")]);
        assert!(matches!(
            Scene::from_package(&corrupt),
            Err(Error::Json { .. })
        ));
    }

    #[test]
    fn project_workshop_ids_accept_numbers_and_strings() {
        let numeric = must(Project::parse(
            br#"{"title":"Rain","type":"scene","workshopid":42,"tags":["weather"],"contentrating":"Everyone"}"#,
        ));
        assert_eq!(numeric.workshopid, Some(WorkshopId::Number(42)));
        assert_eq!(numeric.wallpaper_type.as_deref(), Some("scene"));
        assert_eq!(numeric.tags, vec!["weather".to_owned()]);
        assert_eq!(numeric.title.as_deref(), Some("Rain"));

        let text = must(Project::parse(br#"{"workshopid":"42"}"#));
        assert_eq!(text.workshopid, Some(WorkshopId::Text("42".to_owned())));

        assert!(matches!(
            Project::parse(br#"{"workshopid":-1}"#),
            Err(Error::Json {
                context: "project.json",
                ..
            })
        ));

        let encoded = must(serde_json::to_vec(&numeric));
        assert_eq!(must(Project::parse(&encoded)), numeric);

        let empty = package(&[]);
        assert!(must(Project::from_package(&empty)).is_none());
    }

    fn kind_of(object: &str) -> ElementKind {
        scene_with_object(object).objects[0].kind()
    }

    fn scene_with_object(object: &str) -> Scene {
        let json = format!(r#"{{"camera":{{}},"general":{{}},"objects":[{object}]}}"#);
        must(Scene::parse(json.as_bytes()))
    }

    fn nested_value(levels: usize) -> String {
        let mut value = "1".to_owned();
        for _ in 0..levels {
            value = format!(r#"{{"value":{value}}}"#);
        }
        value
    }

    fn package(entries: &[(&str, &str)]) -> Package {
        let mut directory = Vec::new();
        let mut payload = Vec::new();
        push_sized(&mut directory, "PKGV0001");
        push_u32(
            &mut directory,
            u32::try_from(entries.len()).expect("count fits"),
        );
        for (path, body) in entries {
            push_sized(&mut directory, path);
            push_u32(
                &mut directory,
                u32::try_from(payload.len()).expect("offset fits"),
            );
            push_u32(
                &mut directory,
                u32::try_from(body.len()).expect("length fits"),
            );
            payload.extend_from_slice(body.as_bytes());
        }
        directory.extend_from_slice(&payload);
        must(Package::parse(&directory))
    }
}
