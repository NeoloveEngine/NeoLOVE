//! Scene data model for the visual editor.
//!
//! A [`Scene`] is a list of [`Entity`] nodes. Each entity owns a 2D transform
//! and any number of [`Component`]s. Components are data-driven: a built-in
//! ("core") component is just a kind name plus a list of typed [`Prop`]s that
//! mirror the real engine `core.*` components, so the inspector and the Luau
//! exporter stay in lockstep with the runtime. Scenes are persisted as compact
//! binary `*.neoscene` files, with legacy JSON reads preserved, and can be
//! exported to a runnable `main.luau`.

use std::io::{Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const SCENE_BINARY_MAGIC: &[u8] = b"NEOLSCN1";
const PREFAB_BINARY_MAGIC: &[u8] = b"NEOLPFB1";

/// An RGBA color stored as four bytes in `[r, g, b, a]` order.
pub type Color = [u8; 4];

fn encode_binary_document<T: Serialize>(
    label: &str,
    magic: &[u8],
    value: &T,
) -> Result<Vec<u8>, String> {
    let mut packed = Vec::new();
    value
        .serialize(&mut rmp_serde::Serializer::new(&mut packed).with_struct_map())
        .map_err(|error| format!("failed to serialize {label}: {error}"))?;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&packed)
        .map_err(|error| format!("failed to compress {label}: {error}"))?;
    let compressed = encoder
        .finish()
        .map_err(|error| format!("failed to finish compressing {label}: {error}"))?;

    let mut out = Vec::with_capacity(magic.len() + compressed.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn decode_binary_document<T: DeserializeOwned>(
    label: &str,
    magic: &[u8],
    bytes: &[u8],
) -> Result<T, String> {
    let payload = bytes
        .strip_prefix(magic)
        .ok_or_else(|| format!("{label} is missing binary format header"))?;
    let mut decoder = ZlibDecoder::new(payload);
    let mut packed = Vec::new();
    decoder
        .read_to_end(&mut packed)
        .map_err(|error| format!("failed to decompress {label}: {error}"))?;
    rmp_serde::from_slice(&packed).map_err(|error| format!("failed to parse {label}: {error}"))
}

fn normalize_entities(entities: &mut [Entity]) {
    for entity in entities {
        for component in &mut entity.components {
            normalize_core_component(component);
        }
    }
}

fn finish_loaded_scene(mut scene: Scene) -> Scene {
    normalize_entities(&mut scene.entities);
    scene.next_id = scene.entities.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    scene
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColorKeypoint {
    pub time: f32,
    pub color: Color,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NumberKeypoint {
    pub time: f32,
    pub value: f32,
}

/// A typed property value. This is the single source of truth for how a value
/// is edited in the inspector and written to Luau.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", content = "v")]
pub enum PropValue {
    Number(f32),
    Int(i32),
    Bool(bool),
    Text(String),
    Color(Color),
    /// A one-of-many string value with its allowed options for the dropdown.
    Enum {
        value: String,
        options: Vec<String>,
    },
    /// An image asset path. Exported as `assets.loadImage("...")` so the
    /// runtime receives an ImageHandle rather than a bare string.
    Image(String),
    /// A font asset path. Fonts are consumed as project-relative paths by the
    /// text renderer.
    Font(String),
    /// A sound asset path. Exported as `assets.loadSound("...")` so audio
    /// components receive a SoundHandle.
    Sound(String),
    /// A fragment shader asset path, exported as a runtime ShaderHandle.
    Shader(String),
    /// An animation clip asset path, exported as a runtime AnimationClip table.
    Animation(String),
    /// Colour keypoints sampled over a particle's normalized lifetime.
    ColorSequence(Vec<ColorKeypoint>),
    /// Numeric keypoints sampled over a particle's normalized lifetime.
    NumberSequence(Vec<NumberKeypoint>),
}

impl PropValue {
    /// The Luau literal for this value.
    pub fn to_luau(&self) -> String {
        match self {
            PropValue::Number(n) => fmt_num(*n),
            PropValue::Int(i) => i.to_string(),
            PropValue::Bool(b) => b.to_string(),
            PropValue::Text(s) => format!("\"{}\"", escape_luau(s)),
            PropValue::Color([r, g, b, a]) => {
                if *a == 255 {
                    format!("Color4({r}, {g}, {b})")
                } else {
                    format!("Color4({r}, {g}, {b}, {a})")
                }
            }
            PropValue::Enum { value, .. } => format!("\"{}\"", escape_luau(value)),
            PropValue::Image(s) => format!("assets.loadImage(\"{}\")", escape_luau(s)),
            PropValue::Font(s) => format!("\"{}\"", escape_luau(s)),
            PropValue::Sound(s) => format!("assets.loadSound(\"{}\")", escape_luau(s)),
            PropValue::Shader(s) => format!("shaders.loadFragment(\"{}\")", escape_luau(s)),
            PropValue::Animation(s) => format!("animation.load(\"{}\")", escape_luau(s)),
            PropValue::ColorSequence(keypoints) => format!(
                "{{{}}}",
                keypoints
                    .iter()
                    .map(|keypoint| format!(
                        "{{ time = {}, color = Color4({}, {}, {}, {}) }}",
                        fmt_num(keypoint.time),
                        keypoint.color[0],
                        keypoint.color[1],
                        keypoint.color[2],
                        keypoint.color[3]
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            PropValue::NumberSequence(keypoints) => format!(
                "{{{}}}",
                keypoints
                    .iter()
                    .map(|keypoint| format!(
                        "{{ time = {}, value = {} }}",
                        fmt_num(keypoint.time),
                        fmt_num(keypoint.value)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// A single editable property on a component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Prop {
    /// The runtime field name, e.g. `color` or `align_x`.
    pub name: String,
    /// The inspector label.
    pub label: String,
    pub value: PropValue,
    /// Advanced props are tucked under a collapsible section in the inspector.
    #[serde(default)]
    pub advanced: bool,
    /// Optional props are omitted from the Luau export when left at their empty
    /// default (e.g. an unset font), so the runtime keeps its own default.
    #[serde(default)]
    pub optional: bool,
}

impl Prop {
    fn num(name: &str, label: &str, v: f32) -> Self {
        Self::new(name, label, PropValue::Number(v), false)
    }
    fn num_adv(name: &str, label: &str, v: f32) -> Self {
        Self::new(name, label, PropValue::Number(v), true)
    }
    fn boolean(name: &str, label: &str, v: bool) -> Self {
        Self::new(name, label, PropValue::Bool(v), false)
    }
    fn boolean_adv(name: &str, label: &str, v: bool) -> Self {
        Self::new(name, label, PropValue::Bool(v), true)
    }
    fn text(name: &str, label: &str, v: &str) -> Self {
        Self::new(name, label, PropValue::Text(v.to_string()), false)
    }
    fn color(name: &str, label: &str, v: Color) -> Self {
        Self::new(name, label, PropValue::Color(v), false)
    }
    fn color_adv(name: &str, label: &str, v: Color) -> Self {
        Self::new(name, label, PropValue::Color(v), true)
    }
    fn enumv(name: &str, label: &str, value: &str, options: &[&str], advanced: bool) -> Self {
        Self::new(
            name,
            label,
            PropValue::Enum {
                value: value.to_string(),
                options: options.iter().map(|s| s.to_string()).collect(),
            },
            advanced,
        )
    }
    fn int(name: &str, label: &str, v: i32) -> Self {
        Self::new(name, label, PropValue::Int(v), false)
    }
    fn image(name: &str, label: &str, v: &str) -> Self {
        Self::new(name, label, PropValue::Image(v.to_string()), false)
    }
    fn font(name: &str, label: &str, v: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value: PropValue::Font(v.to_string()),
            advanced: false,
            optional: true,
        }
    }
    fn sound(name: &str, label: &str, v: &str) -> Self {
        Self::new(name, label, PropValue::Sound(v.to_string()), false)
    }
    fn shader(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value: PropValue::Shader(String::new()),
            advanced: true,
            optional: true,
        }
    }
    fn animation(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value: PropValue::Animation(String::new()),
            advanced: false,
            optional: true,
        }
    }
    fn color_sequence(name: &str, label: &str, keypoints: Vec<ColorKeypoint>) -> Self {
        Self::new(name, label, PropValue::ColorSequence(keypoints), false)
    }
    fn number_sequence(name: &str, label: &str, keypoints: Vec<NumberKeypoint>) -> Self {
        Self::new(name, label, PropValue::NumberSequence(keypoints), false)
    }
    fn new(name: &str, label: &str, value: PropValue, advanced: bool) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value,
            advanced,
            optional: false,
        }
    }
}

/// A typed value for a script's public variable (Unity-style serialized field).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum VarValue {
    Number(f32),
    Bool(bool),
    Text(String),
    Color(Color),
    /// A reference to an entity in this scene. `None` is an unassigned
    /// `Inspector(IEntity)` field.
    Entity(Option<u64>),
    /// A reference to a component attached to an entity in this scene.
    Component(Option<ComponentReference>),
    /// Project-relative asset handle fields for custom script Inspector data.
    Image(String),
    Audio(String),
    Shader(String),
    Animation(String),
    List(Vec<VarValue>),
    Dictionary(Vec<DictionaryEntry>),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComponentReference {
    pub entity: u64,
    pub component: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum VarKey {
    Number(f32),
    Bool(bool),
    Text(String),
}

impl VarKey {
    pub fn to_luau(&self) -> String {
        match self {
            Self::Number(value) => fmt_num(*value),
            Self::Bool(value) => value.to_string(),
            Self::Text(value) => format!("\"{}\"", escape_luau(value)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DictionaryEntry {
    pub key: VarKey,
    pub value: VarValue,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum VarControl {
    #[default]
    Field,
    Slider {
        min: f32,
        max: f32,
        fractional: bool,
    },
}

impl VarValue {
    pub fn to_luau(&self) -> String {
        match self {
            VarValue::Number(n) => fmt_num(*n),
            VarValue::Bool(b) => b.to_string(),
            VarValue::Text(s) => format!("\"{}\"", escape_luau(s)),
            VarValue::Color([r, g, b, a]) => {
                if *a == 255 {
                    format!("Color4({r}, {g}, {b})")
                } else {
                    format!("Color4({r}, {g}, {b}, {a})")
                }
            }
            // Scene export resolves references against its generated local
            // variables. Outside that context, an unassigned value is safest.
            VarValue::Entity(_) | VarValue::Component(_) => "nil".to_string(),
            VarValue::Image(path) => {
                if path.is_empty() {
                    "nil".to_string()
                } else {
                    format!("assets.loadImage(\"{}\")", escape_luau(path))
                }
            }
            VarValue::Audio(path) => {
                if path.is_empty() {
                    "nil".to_string()
                } else {
                    format!("assets.loadSound(\"{}\")", escape_luau(path))
                }
            }
            VarValue::Shader(path) => {
                if path.is_empty() {
                    "nil".to_string()
                } else {
                    format!("shaders.loadFragment(\"{}\")", escape_luau(path))
                }
            }
            VarValue::Animation(path) => {
                if path.is_empty() {
                    "nil".to_string()
                } else {
                    format!("animation.load(\"{}\")", escape_luau(path))
                }
            }
            VarValue::List(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(Self::to_luau)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            VarValue::Dictionary(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|entry| format!("[{}] = {}", entry.key.to_luau(), entry.value.to_luau()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn contains_reference(&self) -> bool {
        match self {
            Self::Entity(_) | Self::Component(_) => true,
            Self::List(values) => values.iter().any(Self::contains_reference),
            Self::Dictionary(entries) => {
                entries.iter().any(|entry| entry.value.contains_reference())
            }
            _ => false,
        }
    }

    fn to_luau_with_references(
        &self,
        entities: &std::collections::HashMap<u64, String>,
        components: &std::collections::HashMap<(u64, usize), String>,
    ) -> String {
        match self {
            Self::Entity(Some(id)) => entities.get(id).cloned().unwrap_or_else(|| "nil".into()),
            Self::Entity(None) => "nil".into(),
            Self::Component(Some(reference)) => components
                .get(&(reference.entity, reference.component))
                .cloned()
                .unwrap_or_else(|| "nil".into()),
            Self::Component(None) => "nil".into(),
            Self::List(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|value| value.to_luau_with_references(entities, components))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Dictionary(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|entry| format!(
                        "[{}] = {}",
                        entry.key.to_luau(),
                        entry.value.to_luau_with_references(entities, components)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Image(_) | Self::Audio(_) | Self::Shader(_) | Self::Animation(_) => {
                self.to_luau()
            }
            _ => self.to_luau(),
        }
    }

    fn remap_entity_references(&mut self, map: &std::collections::HashMap<u64, u64>) {
        match self {
            Self::Entity(Some(id)) => {
                if let Some(next) = map.get(id) {
                    *id = *next;
                }
            }
            Self::Component(Some(reference)) => {
                if let Some(next) = map.get(&reference.entity) {
                    reference.entity = *next;
                }
            }
            Self::List(values) => {
                for value in values {
                    value.remap_entity_references(map);
                }
            }
            Self::Dictionary(entries) => {
                for entry in entries {
                    entry.value.remap_entity_references(map);
                }
            }
            _ => {}
        }
    }

    fn remove_entity_reference(&mut self, id: u64) {
        match self {
            Self::Entity(reference) if *reference == Some(id) => *reference = None,
            Self::Component(reference)
                if reference
                    .as_ref()
                    .is_some_and(|reference| reference.entity == id) =>
            {
                *reference = None;
            }
            Self::List(values) => {
                for value in values {
                    value.remove_entity_reference(id);
                }
            }
            Self::Dictionary(entries) => {
                for entry in entries {
                    entry.value.remove_entity_reference(id);
                }
            }
            _ => {}
        }
    }

    fn remove_component_reference(&mut self, entity: u64, removed: usize) {
        match self {
            Self::Component(reference) => {
                if let Some(target) = reference {
                    if target.entity == entity {
                        if target.component == removed {
                            *reference = None;
                        } else if target.component > removed {
                            target.component -= 1;
                        }
                    }
                }
            }
            Self::List(values) => {
                for value in values {
                    value.remove_component_reference(entity, removed);
                }
            }
            Self::Dictionary(entries) => {
                for entry in entries {
                    entry.value.remove_component_reference(entity, removed);
                }
            }
            _ => {}
        }
    }
}

/// A named public variable exposed by a script component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScriptVar {
    pub name: String,
    pub value: VarValue,
    #[serde(default)]
    pub control: VarControl,
}

/// A component attached to an entity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum Component {
    /// A built-in engine component (`core.<name>`) with its editable props.
    Core { name: String, props: Vec<Prop> },
    /// A user-authored Luau component module plus inspector-exposed variables.
    Script {
        path: String,
        variables: Vec<ScriptVar>,
    },
}

impl Component {
    /// A short label for the inspector header.
    pub fn label(&self) -> &str {
        match self {
            Component::Core { name, .. } => name,
            Component::Script { .. } => "Script",
        }
    }

    /// Build a default instance of the named core component.
    pub fn core(name: &str) -> Component {
        Component::Core {
            name: name.to_string(),
            props: core_component_props(name),
        }
    }
}

fn normalize_core_component(component: &mut Component) {
    let Component::Core { name, props } = component else {
        return;
    };
    // Scenes saved before asset pickers had fonts represented as generic text.
    // Upgrade them in place so existing projects get the picker too.
    if matches!(
        name.as_str(),
        "TextBox" | "TextLabel" | "RudimentaryTextLabel"
    ) {
        for prop in props.iter_mut() {
            if prop.name == "font" {
                if let PropValue::Text(path) = &prop.value {
                    prop.value = PropValue::Font(path.clone());
                }
            }
        }
    }

    if name == "ParticleSystem2D" {
        if !props.iter().any(|prop| prop.name == "image") {
            if let Some(image) = core_component_props(name)
                .into_iter()
                .find(|prop| prop.name == "image")
            {
                props.insert(0, image);
            }
        }
        let start = props
            .iter()
            .find(|prop| prop.name == "start_color")
            .and_then(|prop| match prop.value {
                PropValue::Color(color) => Some(color),
                _ => None,
            })
            .unwrap_or([255, 184, 76, 255]);
        let end = props
            .iter()
            .find(|prop| prop.name == "end_color")
            .and_then(|prop| match prop.value {
                PropValue::Color(color) => Some(color),
                _ => None,
            })
            .unwrap_or([255, 92, 40, 0]);
        if !props.iter().any(|prop| prop.name == "color_sequence") {
            props.push(Prop::color_sequence(
                "color_sequence",
                "Color",
                vec![
                    ColorKeypoint {
                        time: 0.0,
                        color: [start[0], start[1], start[2], 255],
                    },
                    ColorKeypoint {
                        time: 1.0,
                        color: [end[0], end[1], end[2], 255],
                    },
                ],
            ));
        }
        if !props
            .iter()
            .any(|prop| prop.name == "transparency_sequence")
        {
            props.push(Prop::number_sequence(
                "transparency_sequence",
                "Transparency",
                vec![
                    NumberKeypoint {
                        time: 0.0,
                        value: 1.0 - start[3] as f32 / 255.0,
                    },
                    NumberKeypoint {
                        time: 1.0,
                        value: 1.0 - end[3] as f32 / 255.0,
                    },
                ],
            ));
        }
        props.retain(|prop| prop.name != "start_color" && prop.name != "end_color");
    }

    if !props.iter().any(|prop| prop.name == "shader") {
        if let Some(shader) = core_component_props(name)
            .into_iter()
            .find(|prop| prop.name == "shader")
        {
            props.push(shader);
        }
    }

    // Components whose editable field set has grown over time. Merge in any
    // default props this editor version knows about that the stored component is
    // missing, so existing scenes gain new fields (e.g. UI hover colours)
    // without losing user-authored values. Ordering follows the current
    // defaults; unknown/forward-compatible fields are preserved at the end.
    if matches!(
        name.as_str(),
        "EntityScaler" | "Panel" | "Frame" | "Button" | "Slider" | "Dropdown" | "TextInput"
    ) {
        let mut existing = std::mem::take(props);
        let mut normalized = Vec::new();
        for default in core_component_props(name) {
            if let Some(index) = existing.iter().position(|prop| prop.name == default.name) {
                normalized.push(existing.remove(index));
            } else {
                normalized.push(default);
            }
        }
        // Preserve forward-compatible or user-authored fields that this editor
        // version does not know about.
        normalized.extend(existing);
        *props = normalized;
    }
}

/// Common core components offered directly in the "Add Component" menu, in
/// display order, matching the engine's `core` module.
pub const CORE_COMPONENTS: &[&str] = &[
    "Rect2D",
    "Shape2D",
    "ParticleSystem2D",
    "SpatialSound2D",
    "TextBox",
    "TextLabel",
    "TextInput",
    "Panel",
    "Button",
    "Slider",
    "Dropdown",
    "Sprite2D",
    "SpriteSheet2D",
    "Image2D",
    "NineSliceSprite2D",
    "Tilemap2D",
    "TileTexture2D",
    "AnimationController",
    "EntityScaler",
    "Collider2D",
    "Rigidbody2D",
];

/// Advanced / legacy core components, shown under an "Advanced" submenu.
pub const ADVANCED_COMPONENTS: &[&str] = &[
    "Spritebox2D",
    "Bolt2D",
    "Rope2D",
    "LegacyBolt2D",
    "String2D",
    "RudimentaryTextLabel",
];

/// Default editable properties for each core component, grounded in the engine
/// API. Read-only/computed fields are intentionally omitted; rarely-touched
/// fields are marked `advanced` so the inspector can collapse them.
pub fn core_component_props(name: &str) -> Vec<Prop> {
    let drawable = || {
        vec![
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::shader("shader", "Shader"),
        ]
    };
    match name {
        "Rect2D" => drawable(),
        "Shape2D" => {
            let mut p = drawable();
            p.push(Prop::enumv(
                "shape",
                "Shape",
                "box",
                &["box", "circle", "triangle", "right_triangle"],
                false,
            ));
            p.push(Prop::enumv(
                "triangle_corner",
                "Tri Corner",
                "bl",
                &["bl", "br", "tl", "tr"],
                true,
            ));
            p.push(Prop::num_adv("offset_x", "Offset X", 0.0));
            p.push(Prop::num_adv("offset_y", "Offset Y", 0.0));
            p.push(Prop::num_adv("size_x", "Size X", 0.0));
            p.push(Prop::num_adv("size_y", "Size Y", 0.0));
            p
        }
        "ParticleSystem2D" => vec![
            Prop::image("image", "Particle Image", ""),
            Prop::boolean("playing", "Playing", true),
            Prop::boolean("looping", "Looping", true),
            Prop::boolean("visible", "Visible", true),
            Prop::num("duration", "Duration", 5.0),
            Prop::num("emission_rate", "Rate", 12.0),
            Prop::int("max_particles", "Max Particles", 256),
            Prop::num("lifetime", "Lifetime", 1.5),
            Prop::num("speed", "Speed", 80.0),
            Prop::num("direction", "Direction °", -90.0),
            Prop::num("spread", "Spread °", 30.0),
            Prop::num("start_size", "Start Size", 8.0),
            Prop::num("end_size", "End Size", 2.0),
            Prop::color_sequence(
                "color_sequence",
                "Color",
                vec![
                    ColorKeypoint {
                        time: 0.0,
                        color: [255, 184, 76, 255],
                    },
                    ColorKeypoint {
                        time: 1.0,
                        color: [255, 92, 40, 255],
                    },
                ],
            ),
            Prop::number_sequence(
                "transparency_sequence",
                "Transparency",
                vec![
                    NumberKeypoint {
                        time: 0.0,
                        value: 0.0,
                    },
                    NumberKeypoint {
                        time: 1.0,
                        value: 1.0,
                    },
                ],
            ),
            Prop::enumv(
                "shape",
                "Emitter",
                "point",
                &["point", "box", "circle"],
                false,
            ),
            Prop::num("radius", "Radius", 32.0),
            Prop::num_adv("gravity_x", "Gravity X", 0.0),
            Prop::num_adv("gravity_y", "Gravity Y", 60.0),
            Prop::shader("shader", "Shader"),
        ],
        "AnimationController" => vec![
            Prop::animation("animation", "Animation"),
            Prop::boolean("autoplay", "Autoplay", true),
            Prop::boolean("looping", "Looping", true),
            Prop::boolean("playing", "Playing", false),
            Prop::num("speed", "Speed", 1.0),
        ],
        "TextBox" | "TextLabel" | "RudimentaryTextLabel" => {
            let mut p = vec![
                Prop::text("text", "Text", "Text"),
                Prop::color("color", "Color", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
                Prop::shader("shader", "Shader"),
                Prop::num("scale", "Scale", 24.0),
                Prop::enumv(
                    "antialiasing",
                    "Anti-aliasing",
                    "inherit",
                    &["inherit", "off", "standard", "high"],
                    false,
                ),
                Prop::font("font", "Font", ""),
                Prop::enumv(
                    "align_x",
                    "Align X",
                    "left",
                    &["left", "center", "right"],
                    false,
                ),
                Prop::enumv(
                    "align_y",
                    "Align Y",
                    "top",
                    &["top", "center", "bottom"],
                    false,
                ),
                Prop::enumv("wrap", "Wrap", "none", &["none", "word", "char"], false),
            ];
            p.push(Prop::num_adv("min_scale", "Min Scale", 1.0));
            p.push(Prop::enumv(
                "text_scale",
                "Text Fit",
                "fit",
                &["none", "fit", "fit_width", "fit_height"],
                false,
            ));
            p.push(Prop::enumv(
                "size_mode",
                "Bounds",
                "entity",
                &["content", "entity", "box"],
                true,
            ));
            p.push(Prop::num_adv("padding", "Padding", 0.0));
            p.push(Prop::num_adv("padding_x", "Padding X", 0.0));
            p.push(Prop::num_adv("padding_y", "Padding Y", 0.0));
            p.push(Prop::num_adv("line_spacing", "Line Space", 1.0));
            p.push(Prop::num_adv("letter_spacing", "Letter Space", 0.0));
            p.push(Prop::num_adv("tab_size", "Tab Size", 4.0));
            p
        }
        "Sprite2D" | "Image2D" => {
            let mut p = vec![
                Prop::image("image", "Image", "assets/sprite.png"),
                Prop::color("color", "Tint", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
                Prop::shader("shader", "Shader"),
            ];
            p.push(Prop::num_adv("source_x", "Source X", 0.0));
            p.push(Prop::num_adv("source_y", "Source Y", 0.0));
            p.push(Prop::num_adv("source_w", "Source W", 0.0));
            p.push(Prop::num_adv("source_h", "Source H", 0.0));
            p
        }
        "SpriteSheet2D" => vec![
            Prop::image("image", "Sprite Sheet", "assets/spritesheet.png"),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::shader("shader", "Shader"),
            Prop::num("frame_width", "Frame W", 32.0),
            Prop::num("frame_height", "Frame H", 32.0),
            Prop::int("frame", "Frame", 0),
            Prop::int("frame_count", "Frame Count", 0),
            Prop::int("columns", "Columns", 0),
            Prop::num_adv("spacing", "Spacing", 0.0),
            Prop::num_adv("margin", "Margin", 0.0),
            Prop::num("fps", "FPS", 12.0),
            Prop::boolean("playing", "Playing", true),
            Prop::boolean("looping", "Looping", true),
        ],
        "NineSliceSprite2D" => {
            let mut p = vec![
                Prop::image("image", "Image", "assets/sprite.png"),
                Prop::color("color", "Tint", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
                Prop::shader("shader", "Shader"),
                Prop::num("slice_left", "Slice L", 8.0),
                Prop::num("slice_right", "Slice R", 8.0),
                Prop::num("slice_top", "Slice T", 8.0),
                Prop::num("slice_bottom", "Slice B", 8.0),
            ];
            p.push(Prop::num_adv("source_x", "Source X", 0.0));
            p.push(Prop::num_adv("source_y", "Source Y", 0.0));
            p
        }
        "TileTexture2D" => vec![
            Prop::image("image", "Image", "assets/tile.png"),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::shader("shader", "Shader"),
            Prop::num("tile_width", "Tile W", 32.0),
            Prop::num("tile_height", "Tile H", 32.0),
            Prop::num_adv("offset_x", "Offset X", 0.0),
            Prop::num_adv("offset_y", "Offset Y", 0.0),
        ],
        "Tilemap2D" => vec![
            Prop::image("image", "Tileset", "assets/tiles.png"),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::shader("shader", "Shader"),
            Prop::int("map_width", "Columns", 10),
            Prop::int("map_height", "Rows", 10),
            Prop::num("tile_width", "Tile W", 32.0),
            Prop::num("tile_height", "Tile H", 32.0),
            Prop::text("tiles", "Tile IDs", "0"),
            Prop::num_adv("spacing", "Spacing", 0.0),
            Prop::num_adv("margin", "Margin", 0.0),
        ],
        "EntityScaler" => vec![
            Prop::boolean("enabled", "Enabled", true),
            Prop::boolean("edit_with_percent", "Edit With %", true),
            Prop::num("x_percent", "X %", 0.0),
            Prop::num("y_percent", "Y %", 0.0),
            Prop::num("size_x_percent", "Size X %", 0.0),
            Prop::num("size_y_percent", "Size Y %", 0.0),
            Prop::num("offset_x", "Offset X", 0.0),
            Prop::num("offset_y", "Offset Y", 0.0),
            Prop::num("pivot_x", "Pivot X", 0.0),
            Prop::num("pivot_y", "Pivot Y", 0.0),
        ],
        "Collider2D" => vec![
            Prop::boolean("enabled", "Enabled", true),
            Prop::boolean("is_trigger", "Is Trigger", false),
            Prop::enumv(
                "shape",
                "Shape",
                "box",
                &["box", "circle", "triangle"],
                false,
            ),
            Prop::num("size_x", "Size X", 0.0),
            Prop::num("size_y", "Size Y", 0.0),
            Prop::num_adv("offset_x", "Offset X", 0.0),
            Prop::num_adv("offset_y", "Offset Y", 0.0),
            Prop::num_adv("restitution", "Restitution", 0.0),
            Prop::num_adv("friction", "Friction", 0.5),
            Prop::boolean_adv("non_physics", "Non Physics", false),
        ],
        "Rigidbody2D" => vec![
            Prop::boolean("is_static", "Static", false),
            Prop::num("mass", "Mass", 1.0),
            Prop::num("gravity_scale", "Gravity Scale", 1.0),
            Prop::num("linear_damping", "Lin Damping", 0.0),
            Prop::num("angular_damping", "Ang Damping", 0.0),
            Prop::boolean("freeze_rotation", "Freeze Rot", false),
            Prop::boolean_adv("freeze_x", "Freeze X", false),
            Prop::boolean_adv("freeze_y", "Freeze Y", false),
            Prop::num_adv("restitution", "Restitution", 0.0),
            Prop::num_adv("friction", "Friction", 0.5),
            Prop::num_adv("max_speed", "Max Speed", 0.0),
        ],
        "SpatialSound2D" => vec![
            Prop::sound("sound", "Sound", ""),
            Prop::num("volume", "Volume", 1.0),
            Prop::boolean("looping", "Looping", false),
            Prop::boolean("autoplay", "Autoplay", false),
        ],
        "Bolt2D" => vec![
            Prop::boolean("enabled", "Enabled", true),
            Prop::num("strength", "Strength", 1.0),
            Prop::num("offset_x", "Offset X", 0.0),
            Prop::num("offset_y", "Offset Y", 0.0),
            Prop::boolean_adv("contacts_enabled", "Contacts", true),
        ],
        "Rope2D" | "String2D" => vec![
            Prop::boolean("enabled", "Enabled", true),
            Prop::num("min_length", "Min Length", 0.0),
            Prop::num("max_length", "Max Length", 100.0),
            Prop::num("stiffness", "Stiffness", 1.0),
            Prop::num("damping", "Damping", 0.0),
            Prop::num_adv("break_force", "Break Force", 0.0),
        ],
        "LegacyBolt2D" => core_component_props("Bolt2D"),
        // ---- UI components ----
        "Frame" | "Panel" => vec![
            Prop::color("background_color", "Background", [37, 37, 38, 255]),
            Prop::color("border_color", "Border Color", [69, 69, 69, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::num("corner_radius", "Corner", 4.0),
            Prop::num_adv("border_width", "Border", 1.0),
            Prop::image("background_image", "Background Image", ""),
            Prop::num_adv("slice_left", "Slice L", 0.0),
            Prop::num_adv("slice_right", "Slice R", 0.0),
            Prop::num_adv("slice_top", "Slice T", 0.0),
            Prop::num_adv("slice_bottom", "Slice B", 0.0),
        ],
        "Slider" => vec![
            Prop::num("min", "Min", 0.0),
            Prop::num("max", "Max", 100.0),
            Prop::num("value", "Value", 0.0),
            Prop::num("step", "Step", 0.0),
            Prop::color("fill_color", "Fill", [0, 122, 204, 255]),
            Prop::color("hover_fill_color", "Fill Hover", [17, 119, 187, 255]),
            Prop::color("background_color", "Track", [60, 60, 60, 255]),
            Prop::color("hover_background_color", "Track Hover", [66, 66, 66, 255]),
            Prop::color("thumb_color", "Thumb", [204, 204, 204, 255]),
            Prop::color("hover_thumb_color", "Thumb Hover", [255, 255, 255, 255]),
            // Alpha of any colour below the 255 default makes it translucent.
            Prop::color_adv("disabled_fill_color", "Fill Off", [60, 60, 60, 180]),
            Prop::color_adv("disabled_background_color", "Track Off", [60, 60, 60, 120]),
            Prop::color_adv("disabled_thumb_color", "Thumb Off", [128, 128, 128, 255]),
            Prop::color_adv("border_color", "Track Border", [60, 60, 60, 255]),
            Prop::color_adv("hover_border_color", "Border Hover", [98, 98, 98, 255]),
            Prop::color_adv("disabled_border_color", "Border Off", [60, 60, 60, 120]),
            Prop::boolean("visible", "Visible", true),
            Prop::boolean("enabled", "Enabled", true),
            Prop::enumv(
                "orientation",
                "Orientation",
                "horizontal",
                &["horizontal", "vertical"],
                false,
            ),
            Prop::num("thumb_size", "Thumb Size", 16.0),
            Prop::num_adv("track_thickness", "Track Thick", 6.0),
            Prop::num_adv("corner_radius", "Corner", 3.0),
            Prop::num_adv("thumb_corner_radius", "Thumb Corner", 8.0),
        ],
        "Button" => vec![
            Prop::text("text", "Text", "Button"),
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("scale", "Scale", 18.0),
            Prop::enumv(
                "align_x",
                "Align X",
                "center",
                &["left", "center", "right"],
                false,
            ),
            Prop::color("background_color", "Background", [14, 99, 156, 255]),
            Prop::color("hover_background_color", "Bg Hover", [17, 119, 187, 255]),
            Prop::color("text_color", "Text Color", [255, 255, 255, 255]),
            // Extra states — each swatch opens a picker with an alpha (A) slider
            // for transparency.
            Prop::color_adv("pressed_background_color", "Bg Pressed", [10, 76, 121, 255]),
            Prop::color_adv("disabled_background_color", "Bg Off", [37, 37, 38, 190]),
            Prop::color_adv("hover_text_color", "Text Hover", [255, 255, 255, 255]),
            Prop::color_adv("pressed_text_color", "Text Pressed", [255, 255, 255, 255]),
            Prop::color_adv("disabled_text_color", "Text Off", [204, 204, 204, 120]),
            Prop::color_adv("border_color", "Border Color", [14, 99, 156, 255]),
            Prop::color_adv("hover_border_color", "Border Hover", [17, 119, 187, 255]),
            Prop::color_adv("pressed_border_color", "Border Pressed", [10, 76, 121, 255]),
            Prop::color_adv("disabled_border_color", "Border Off", [37, 37, 38, 190]),
            Prop::num("corner_radius", "Corner", 2.0),
            Prop::num_adv("border_width", "Border", 0.0),
            Prop::num_adv("padding_x", "Padding X", 12.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
            Prop::num_adv("icon_gap", "Icon Gap", 8.0),
        ],
        "TextInput" => vec![
            Prop::text("text", "Text", ""),
            Prop::text("placeholder", "Placeholder", "Type here"),
            Prop::color("text_color", "Text Color", [204, 204, 204, 255]),
            Prop::color(
                "placeholder_color",
                "Placeholder Color",
                [166, 166, 166, 255],
            ),
            Prop::color("caret_color", "Caret Color", [174, 175, 173, 255]),
            Prop::color("background_color", "Background", [60, 60, 60, 255]),
            Prop::color("border_color", "Border Color", [60, 60, 60, 255]),
            Prop::color("hover_border_color", "Border Hover", [98, 98, 98, 255]),
            Prop::color("focus_border_color", "Focus Border", [0, 127, 212, 255]),
            Prop::color_adv("hover_background_color", "Bg Hover", [66, 66, 66, 255]),
            Prop::color_adv("focus_background_color", "Bg Focus", [60, 60, 60, 255]),
            Prop::color_adv("disabled_background_color", "Bg Off", [60, 60, 60, 120]),
            Prop::color_adv("disabled_border_color", "Border Off", [60, 60, 60, 120]),
            Prop::color_adv("disabled_text_color", "Text Off", [204, 204, 204, 120]),
            Prop::boolean("visible", "Visible", true),
            Prop::boolean("enabled", "Enabled", true),
            Prop::boolean("locked", "Locked", false),
            Prop::num("scale", "Scale", 18.0),
            Prop::num_adv("min_scale", "Min Scale", 12.0),
            Prop::font("font", "Font", ""),
            Prop::enumv(
                "align_x",
                "Align X",
                "left",
                &["left", "center", "right"],
                false,
            ),
            Prop::enumv(
                "align_y",
                "Align Y",
                "center",
                &["top", "center", "bottom"],
                false,
            ),
            Prop::enumv(
                "antialiasing",
                "Anti-aliasing",
                "inherit",
                &["inherit", "off", "standard", "high"],
                false,
            ),
            Prop::num("corner_radius", "Corner", 2.0),
            Prop::int("max_length", "Max Length", 0),
            Prop::boolean("password", "Password", false),
            Prop::boolean_adv("submit_on_enter", "Submit Enter", true),
            Prop::boolean_adv("clear_on_submit", "Clear Submit", false),
            Prop::boolean_adv("blur_on_submit", "Blur Submit", false),
            Prop::num_adv("border_width", "Border", 1.0),
            Prop::num_adv("caret_width", "Caret Width", 2.0),
            Prop::num_adv("padding_x", "Padding X", 10.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
            Prop::num_adv("letter_spacing", "Letter Space", 0.0),
        ],
        "Dropdown" => vec![
            Prop::color("background_color", "Background", [60, 60, 60, 255]),
            Prop::color("hover_background_color", "Bg Hover", [74, 74, 74, 255]),
            Prop::color("text_color", "Text Color", [240, 240, 240, 255]),
            Prop::color("item_hover_background_color", "Item Hover", [42, 45, 46, 255]),
            Prop::color("item_selected_background_color", "Item Selected", [9, 71, 113, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("scale", "Scale", 18.0),
            Prop::num("item_height", "Item H", 32.0),
            Prop::int("max_visible_items", "Max Items", 8),
            Prop::num("corner_radius", "Corner", 2.0),
            // Extra state / menu colours — each swatch's picker has an alpha (A)
            // slider for transparency.
            Prop::color_adv("open_background_color", "Bg Open", [60, 60, 60, 255]),
            Prop::color_adv("disabled_background_color", "Bg Off", [60, 60, 60, 120]),
            Prop::color_adv("border_color", "Border Color", [69, 69, 69, 255]),
            Prop::color_adv("hover_border_color", "Border Hover", [98, 98, 98, 255]),
            Prop::color_adv("open_border_color", "Border Open", [0, 127, 212, 255]),
            Prop::color_adv("disabled_border_color", "Border Off", [69, 69, 69, 120]),
            Prop::color_adv("disabled_text_color", "Text Off", [204, 204, 204, 120]),
            Prop::color_adv("menu_background_color", "Menu Bg", [37, 37, 38, 255]),
            Prop::color_adv("menu_border_color", "Menu Border", [69, 69, 69, 255]),
            Prop::color_adv("item_background_color", "Item Bg", [37, 37, 38, 0]),
            Prop::color_adv("item_text_color", "Item Text", [204, 204, 204, 255]),
            Prop::color_adv("item_hover_text_color", "Item Hover Text", [255, 255, 255, 255]),
            Prop::color_adv("item_selected_text_color", "Item Sel Text", [255, 255, 255, 255]),
            Prop::boolean_adv("open_upwards", "Open Up", false),
            Prop::num_adv("padding_x", "Padding X", 10.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
        ],
        "ScrollList" => vec![
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("item_height", "Item H", 32.0),
            Prop::num("item_spacing", "Item Gap", 4.0),
            Prop::boolean("show_scrollbar", "Scrollbar", true),
            Prop::num("scrollbar_width", "Bar Width", 8.0),
            Prop::num_adv("padding_x", "Padding X", 10.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
        ],
        "Spritebox2D" => vec![Prop::num("alpha_threshold", "Alpha Thresh", 0.5)],
        _ => Vec::new(),
    }
}

/// A single object in the scene.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: u64,
    pub name: String,
    /// Project-relative prefab source for linked instances. Only the root of
    /// an instantiated prefab carries this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefab_source: Option<String>,
    pub x: f32,
    pub y: f32,
    /// Draw order; higher draws in front.
    #[serde(default)]
    pub z: f32,
    pub size_x: f32,
    pub size_y: f32,
    pub rotation: f32,
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default)]
    pub anchor_x: f32,
    #[serde(default)]
    pub anchor_y: f32,
    /// Named position pivot. Empty means the runtime default, top-left.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub position_pivot: String,
    /// Optional numeric position pivot fractions. When present, these override
    /// `position_pivot`; rotation falls back to them unless it has its own
    /// numeric pivot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot_y: Option<f32>,
    /// Named rotation pivot. Empty means the runtime default, top-left.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rotation_pivot: String,
    /// Optional numeric rotation pivot fractions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_pivot_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_pivot_y: Option<f32>,
    /// Optional parent entity id for hierarchy nesting.
    #[serde(default)]
    pub parent: Option<u64>,
    /// Active entities are exported and drawn solid; inactive ones are skipped
    /// on export and dimmed in the viewport (like Unity's GameObject checkbox).
    #[serde(default = "tru")]
    pub enabled: bool,
    pub components: Vec<Component>,
}

fn one() -> f32 {
    1.0
}

fn tru() -> bool {
    true
}

impl Entity {
    pub fn new(id: u64, name: impl Into<String>, x: f32, y: f32) -> Self {
        Self {
            id,
            name: name.into(),
            prefab_source: None,
            x,
            y,
            z: 0.0,
            size_x: 100.0,
            size_y: 100.0,
            rotation: 0.0,
            scale: 1.0,
            anchor_x: 0.0,
            anchor_y: 0.0,
            position_pivot: String::new(),
            pivot_x: None,
            pivot_y: None,
            rotation_pivot: String::new(),
            rotation_pivot_x: None,
            rotation_pivot_y: None,
            parent: None,
            enabled: true,
            components: Vec::new(),
        }
    }
}

/// The complete editable document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub background: Color,
    /// When true (the default) textures are upscaled with nearest-neighbour
    /// sampling for crisp pixel-art; when false they use bilinear filtering for
    /// a smoother look. Exported as `app.nearestNeighborScaling`.
    #[serde(default = "default_nearest_neighbor")]
    pub nearest_neighbor_scaling: bool,
    /// Geometry and default text anti-aliasing quality: off, standard, or high.
    #[serde(default = "default_antialiasing")]
    pub antialiasing: String,
    pub entities: Vec<Entity>,
    #[serde(skip)]
    next_id: u64,
}

const DEFAULT_SCENE_BACKGROUND: Color = [20, 20, 20, 255];

fn default_nearest_neighbor() -> bool {
    true
}

fn default_antialiasing() -> String {
    "high".to_string()
}

impl Default for Scene {
    fn default() -> Self {
        let mut scene = Self {
            name: "Untitled".to_string(),
            background: DEFAULT_SCENE_BACKGROUND,
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities: Vec::new(),
            next_id: 1,
        };
        // A fresh scene starts with one empty entity so the viewport is not
        // blank — but with no components attached.
        scene.add_entity("Entity", 200.0, 150.0);
        scene
    }
}

/// How a generated scene should obtain its shared image cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageEmitMode {
    /// `require("./images")` — the exported start scene's cache module written
    /// next to `main.luau`.
    SharedModule,
    /// Inline `assets.loadImage(...)` calls, so the scene is self-contained.
    Inline,
}

impl Scene {
    pub fn from_prefab(name: impl Into<String>, mut entities: Vec<Entity>) -> Self {
        for entity in &mut entities {
            entity.prefab_source = None;
            for component in &mut entity.components {
                normalize_core_component(component);
            }
        }
        let next_id = entities.iter().map(|entity| entity.id).max().unwrap_or(0) + 1;
        Self {
            name: name.into(),
            background: DEFAULT_SCENE_BACKGROUND,
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities,
            next_id,
        }
    }

    pub fn add_entity(&mut self, name: impl Into<String>, x: f32, y: f32) -> Entity {
        let id = self.allocate_id();
        let entity = Entity::new(id, name, x, y);
        self.entities.push(entity.clone());
        entity
    }

    /// Insert a fully-formed entity (used by paste/duplicate), assigning a new id.
    pub fn insert_entity(&mut self, mut entity: Entity) -> u64 {
        let id = self.allocate_id();
        entity.id = id;
        for component in &mut entity.components {
            normalize_core_component(component);
        }
        self.entities.push(entity);
        id
    }

    fn allocate_id(&mut self) -> u64 {
        let highest = self.entities.iter().map(|e| e.id).max().unwrap_or(0);
        if self.next_id <= highest {
            self.next_id = highest + 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn entity(&self, id: u64) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    pub fn entity_mut(&mut self, id: u64) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    pub fn replace_entity(&mut self, id: u64, value: Entity) {
        if let Some(slot) = self.entities.iter_mut().find(|e| e.id == id) {
            *slot = value;
        }
    }

    /// Remove an entity and reparent any of its children to the root.
    pub fn remove_entity(&mut self, id: u64) {
        for e in self.entities.iter_mut() {
            if e.parent == Some(id) {
                e.parent = None;
            }
        }
        self.entities.retain(|e| e.id != id);
        for entity in &mut self.entities {
            for component in &mut entity.components {
                if let Component::Script { variables, .. } = component {
                    for variable in variables {
                        variable.value.remove_entity_reference(id);
                    }
                }
            }
        }
    }

    /// Clear references to a removed component and shift references to later
    /// components on the same entity so they continue to point at their target.
    pub fn adjust_component_references(&mut self, entity: u64, removed: usize) {
        for owner in &mut self.entities {
            for component in &mut owner.components {
                if let Component::Script { variables, .. } = component {
                    for variable in variables {
                        variable.value.remove_component_reference(entity, removed);
                    }
                }
            }
        }
    }

    /// Would making `child` a descendant of `new_parent` create a cycle?
    pub fn would_cycle(&self, child: u64, new_parent: u64) -> bool {
        if child == new_parent {
            return true;
        }
        let mut cur = Some(new_parent);
        while let Some(id) = cur {
            if id == child {
                return true;
            }
            cur = self.entity(id).and_then(|e| e.parent);
        }
        false
    }

    /// Instantiate a prefab (a list of entities, root first with no parent)
    /// into the scene with fresh, unique ids and remapped parents. Returns the
    /// new id of the prefab root.
    pub fn instantiate(&mut self, proto: Vec<Entity>) -> Option<u64> {
        let mut map = std::collections::HashMap::new();
        for e in &proto {
            map.insert(e.id, self.allocate_id());
        }
        let mut root = None;
        for mut e in proto {
            let nid = *map.get(&e.id)?;
            e.parent = e.parent.and_then(|p| map.get(&p).copied());
            if e.parent.is_none() && root.is_none() {
                root = Some(nid);
            }
            e.id = nid;
            for component in &mut e.components {
                normalize_core_component(component);
                if let Component::Script { variables, .. } = component {
                    for variable in variables {
                        variable.value.remap_entity_references(&map);
                    }
                }
            }
            self.entities.push(e);
        }
        root
    }

    /// Instantiate and link the new root to its project-relative prefab file.
    pub fn instantiate_linked(
        &mut self,
        proto: Vec<Entity>,
        source: impl Into<String>,
    ) -> Option<u64> {
        let root = self.instantiate(proto)?;
        self.entity_mut(root)?.prefab_source = Some(source.into());
        Some(root)
    }

    /// Replace every linked instance of `source` with the latest prefab data.
    /// Root identity and placement are preserved so external references and
    /// per-scene positioning continue to work.
    pub fn refresh_prefab_instances(&mut self, source: &str, proto: &[Entity]) -> usize {
        let Some(proto_root) = proto.iter().find(|entity| entity.parent.is_none()).cloned() else {
            return 0;
        };
        let roots: Vec<u64> = self
            .entities
            .iter()
            .filter(|entity| entity.prefab_source.as_deref() == Some(source))
            .map(|entity| entity.id)
            .collect();
        let mut refreshed = 0;
        for root_id in roots {
            let Some(instance_root) = self.entity(root_id).cloned() else {
                continue;
            };
            let removed: std::collections::HashSet<u64> = self
                .subtree(root_id)
                .into_iter()
                .map(|entity| entity.id)
                .collect();
            self.entities.retain(|entity| !removed.contains(&entity.id));

            let mut id_map = std::collections::HashMap::new();
            id_map.insert(proto_root.id, root_id);
            for entity in proto.iter().filter(|entity| entity.id != proto_root.id) {
                id_map.insert(entity.id, self.allocate_id());
            }
            for mut entity in proto.iter().cloned() {
                let old_id = entity.id;
                entity.id = id_map[&old_id];
                entity.parent = entity
                    .parent
                    .and_then(|parent| id_map.get(&parent).copied());
                entity.prefab_source = None;
                for component in &mut entity.components {
                    normalize_core_component(component);
                    if let Component::Script { variables, .. } = component {
                        for variable in variables {
                            variable.value.remap_entity_references(&id_map);
                        }
                    }
                }
                if old_id == proto_root.id {
                    entity.parent = instance_root.parent;
                    entity.x = instance_root.x;
                    entity.y = instance_root.y;
                    entity.z = instance_root.z;
                    entity.rotation = instance_root.rotation;
                    entity.scale = instance_root.scale;
                    entity.anchor_x = instance_root.anchor_x;
                    entity.anchor_y = instance_root.anchor_y;
                    entity.prefab_source = Some(source.to_string());
                }
                self.entities.push(entity);
            }
            refreshed += 1;
        }
        refreshed
    }

    /// Collect an entity and all of its descendants, with the root's parent
    /// cleared, for saving as a self-contained prefab.
    pub fn subtree(&self, id: u64) -> Vec<Entity> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            if let Some(e) = self.entity(cur) {
                let mut clone = e.clone();
                if cur == id {
                    clone.parent = None;
                }
                out.push(clone);
                stack.extend(self.children_of(Some(cur)));
            }
        }
        out
    }

    /// Direct children of `parent` (or roots when `parent` is `None`).
    pub fn children_of(&self, parent: Option<u64>) -> Vec<u64> {
        self.entities
            .iter()
            .filter(|e| e.parent == parent)
            .map(|e| e.id)
            .collect()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("failed to serialize scene: {e}"))
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        encode_binary_document("scene", SCENE_BINARY_MAGIC, self)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let scene: Scene =
            serde_json::from_str(text).map_err(|e| format!("failed to parse scene: {e}"))?;
        Ok(finish_loaded_scene(scene))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.starts_with(SCENE_BINARY_MAGIC) {
            let scene: Scene = decode_binary_document("scene", SCENE_BINARY_MAGIC, bytes)?;
            return Ok(finish_loaded_scene(scene));
        }

        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("failed to read scene as UTF-8 JSON: {error}"))?;
        Self::from_json(text)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_bytes()?)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Generate a runnable `main.luau` reconstructing this scene. The image
    /// cache is pulled from the shared `./images` module written alongside
    /// `main.luau`, so the exported start scene loads each image once.
    pub fn to_luau(&self) -> String {
        self.to_luau_with_image_mode(ImageEmitMode::SharedModule)
    }

    /// Like [`Scene::to_luau`], but self-contained: the image cache is inlined
    /// rather than required from the shared `./images` module. Used by the
    /// runtime `loadScene`, which can load any scene in the project — including
    /// scenes whose images are absent from the exported start scene's
    /// `images.luau` (or when that module was never written because the start
    /// scene had no images at all). Inlining keeps each loaded scene's images
    /// present without depending on the exported cache. `assets.loadImage`
    /// caches by path, so re-loading a shared image stays cheap.
    pub fn to_luau_runtime(&self) -> String {
        self.to_luau_with_image_mode(ImageEmitMode::Inline)
    }

    fn to_luau_with_image_mode(&self, image_mode: ImageEmitMode) -> String {
        let mut out = String::new();
        out.push_str("-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\n");
        out.push_str(&format!("-- Scene: {}\n\n", self.name));

        // Require code modules at the top of main.luau. Images remain in their
        // own generated cache module because loading/retaining image handles is
        // asset work, while component code belongs in the entry module.
        let (image_paths, script_paths) = self.collect_assets();
        if !image_paths.is_empty() {
            match image_mode {
                ImageEmitMode::SharedModule => {
                    out.push_str("local Images = require(\"./images\")\n");
                }
                ImageEmitMode::Inline => {
                    out.push_str("local Images = {}\n");
                    for path in &image_paths {
                        let escaped = escape_luau(path);
                        out.push_str(&format!(
                            "Images[\"{escaped}\"] = assets.loadImage(\"{escaped}\")\n"
                        ));
                    }
                }
            }
        }
        let script_vars: std::collections::HashMap<String, String> = script_paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let variable = format!("ScriptModule_{index}");
                out.push_str(&format!(
                    "local {variable} = require(\"{}\")\n",
                    escape_luau(path)
                ));
                (path.clone(), variable)
            })
            .collect();
        if !image_paths.is_empty() || !script_paths.is_empty() {
            out.push('\n');
        }

        let [br, bg, bb, _] = self.background;
        out.push_str(&format!("app.bg = Color4({br}, {bg}, {bb})\n"));
        out.push_str(&format!(
            "app.nearestNeighborScaling = {}\n",
            self.nearest_neighbor_scaling
        ));
        out.push_str(&format!(
            "app.antiAliasing = \"{}\"\n\n",
            escape_luau(&self.antialiasing)
        ));

        // Emit parents before children so `ecs.newEntity(..., parentVar)` works.
        let ordered = self.topological_order();
        let var_of: std::collections::HashMap<u64, String> = ordered
            .iter()
            .enumerate()
            .filter(|(_, id)| self.is_active_in_tree(**id))
            .map(|(index, id)| (*id, format!("ent_{index}")))
            .collect();
        let component_vars: std::collections::HashMap<(u64, usize), String> = self
            .entities
            .iter()
            .filter_map(|entity| {
                var_of
                    .get(&entity.id)
                    .map(|variable| (entity, variable.clone()))
            })
            .flat_map(|(entity, variable)| {
                (0..entity.components.len())
                    .map(move |index| ((entity.id, index), format!("{variable}_c{index}")))
            })
            .collect();
        let mut deferred_reference_assignments = Vec::new();
        for (index, id) in ordered.iter().enumerate() {
            let Some(entity) = self.entity(*id) else {
                continue;
            };
            // Skip inactive entities and anything beneath an inactive ancestor.
            if !self.is_active_in_tree(*id) {
                continue;
            }
            let var = format!("ent_{index}");
            let parent_expr = entity
                .parent
                .and_then(|pid| var_of.get(&pid))
                .map(|v| v.as_str())
                .unwrap_or("ecs.root");
            out.push_str(&format!(
                "local {var} = ecs.newEntity(\"{}\", {parent_expr}, {}, {})\n",
                escape_luau(&entity.name),
                fmt_num(entity.x),
                fmt_num(entity.y),
            ));
            out.push_str(&format!("{var}.size_x = {}\n", fmt_num(entity.size_x)));
            out.push_str(&format!("{var}.size_y = {}\n", fmt_num(entity.size_y)));
            if entity.z != 0.0 {
                out.push_str(&format!("{var}.z = {}\n", fmt_num(entity.z)));
            }
            if entity.rotation != 0.0 {
                out.push_str(&format!("{var}.rotation = {}\n", fmt_num(entity.rotation)));
            }
            if entity.scale != 1.0 {
                out.push_str(&format!("{var}.scale = {}\n", fmt_num(entity.scale)));
            }
            if entity.anchor_x != 0.0 {
                out.push_str(&format!("{var}.anchor_x = {}\n", fmt_num(entity.anchor_x)));
            }
            if entity.anchor_y != 0.0 {
                out.push_str(&format!("{var}.anchor_y = {}\n", fmt_num(entity.anchor_y)));
            }
            if !entity.position_pivot.trim().is_empty() {
                out.push_str(&format!(
                    "{var}.position_pivot = \"{}\"\n",
                    escape_luau(entity.position_pivot.trim())
                ));
            }
            if let Some(pivot_x) = entity.pivot_x.filter(|value| value.is_finite()) {
                out.push_str(&format!("{var}.pivot_x = {}\n", fmt_num(pivot_x)));
            }
            if let Some(pivot_y) = entity.pivot_y.filter(|value| value.is_finite()) {
                out.push_str(&format!("{var}.pivot_y = {}\n", fmt_num(pivot_y)));
            }
            if !entity.rotation_pivot.trim().is_empty() {
                out.push_str(&format!(
                    "{var}.rotation_pivot = \"{}\"\n",
                    escape_luau(entity.rotation_pivot.trim())
                ));
            }
            if let Some(pivot_x) = entity.rotation_pivot_x.filter(|value| value.is_finite()) {
                out.push_str(&format!("{var}.rotation_pivot_x = {}\n", fmt_num(pivot_x)));
            }
            if let Some(pivot_y) = entity.rotation_pivot_y.filter(|value| value.is_finite()) {
                out.push_str(&format!("{var}.rotation_pivot_y = {}\n", fmt_num(pivot_y)));
            }

            for (ci, component) in entity.components.iter().enumerate() {
                let cvar = format!("{var}_c{ci}");
                match component {
                    Component::Core { name, props } => {
                        out.push_str(&format!("local {cvar} = {var}:AddComponent(core.{name})\n"));
                        for prop in props {
                            // Skip optional asset/text props left empty, and
                            // empty handle paths, so the runtime keeps defaults.
                            if prop.optional
                                && matches!(
                                    &prop.value,
                                    PropValue::Text(path)
                                        | PropValue::Font(path)
                                        | PropValue::Image(path)
                                        | PropValue::Sound(path)
                                        | PropValue::Shader(path)
                                        | PropValue::Animation(path)
                                        if path.is_empty()
                                )
                            {
                                continue;
                            }
                            if let PropValue::Image(p) = &prop.value {
                                if p.is_empty() {
                                    continue;
                                }
                                // Reference the shared, pre-loaded handle.
                                out.push_str(&format!(
                                    "{cvar}.{} = Images[\"{}\"]\n",
                                    sanitize_field(&prop.name),
                                    escape_luau(p)
                                ));
                                continue;
                            }
                            if let PropValue::Sound(path) = &prop.value {
                                if path.is_empty() {
                                    continue;
                                }
                            }
                            out.push_str(&format!(
                                "{cvar}.{} = {}\n",
                                sanitize_field(&prop.name),
                                prop.value.to_luau()
                            ));
                        }
                    }
                    Component::Script { path, variables } => {
                        let module = if path.is_empty() {
                            "-- TODO: set script path".to_string()
                        } else {
                            script_vars
                                .get(&normalize_require_path(path))
                                .cloned()
                                .unwrap_or_else(|| "nil -- missing script module".to_string())
                        };
                        out.push_str(&format!("local {cvar} = {var}:AddComponent({module})\n"));
                        for variable in variables {
                            if variable.name.is_empty() {
                                continue;
                            }
                            let assignment = format!(
                                "{cvar}.{} = {}\n",
                                sanitize_field(&variable.name),
                                if variable.value.contains_reference() {
                                    variable
                                        .value
                                        .to_luau_with_references(&var_of, &component_vars)
                                } else {
                                    variable.value.to_luau()
                                }
                            );
                            if variable.value.contains_reference() {
                                deferred_reference_assignments.push(assignment);
                            } else {
                                out.push_str(&assignment);
                            }
                        }
                    }
                }
            }
            out.push('\n');
        }
        if !deferred_reference_assignments.is_empty() {
            out.push_str("-- Inspector scene references\n");
            for assignment in deferred_reference_assignments {
                out.push_str(&assignment);
            }
        }
        out
    }

    /// Unique image paths and script module paths referenced by active
    /// entities, each in first-use order.
    fn collect_assets(&self) -> (Vec<String>, Vec<String>) {
        let mut images: Vec<String> = Vec::new();
        let mut scripts: Vec<String> = Vec::new();
        for id in self.topological_order() {
            if !self.is_active_in_tree(id) {
                continue;
            }
            let Some(entity) = self.entity(id) else {
                continue;
            };
            for component in &entity.components {
                match component {
                    Component::Core { props, .. } => {
                        for prop in props {
                            if let PropValue::Image(path) = &prop.value {
                                if !path.is_empty() && !images.contains(path) {
                                    images.push(path.clone());
                                }
                            }
                        }
                    }
                    Component::Script { path, .. } => {
                        if !path.is_empty() {
                            let module = normalize_require_path(path);
                            if !scripts.contains(&module) {
                                scripts.push(module);
                            }
                        }
                    }
                }
            }
        }
        (images, scripts)
    }

    /// Generate `images.luau`, loading each unique image exactly once. Script
    /// modules are required at the top of `main.luau` instead.
    pub fn to_images_luau(&self) -> Option<String> {
        let (images, _) = self.collect_assets();
        if images.is_empty() {
            return None;
        }

        let mut out = String::new();
        out.push_str("-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\n");
        out.push_str("-- Shared image cache: every image is loaded once and reused.\n\n");
        out.push_str("local Images = {}\n\n");

        for path in &images {
            let escaped = escape_luau(path);
            out.push_str(&format!(
                "Images[\"{escaped}\"] = assets.loadImage(\"{escaped}\")\n"
            ));
        }

        out.push_str("\nreturn Images\n");
        Some(out)
    }

    /// True if this entity and all of its ancestors are enabled.
    pub fn is_active_in_tree(&self, id: u64) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            match self.entity(c) {
                Some(e) if e.enabled => cur = e.parent,
                _ => return false,
            }
        }
        true
    }

    /// Entity ids ordered so every parent precedes its children.
    fn topological_order(&self) -> Vec<u64> {
        let mut out = Vec::new();
        let mut stack: Vec<u64> = self.children_of(None);
        stack.reverse();
        while let Some(id) = stack.pop() {
            out.push(id);
            let mut kids = self.children_of(Some(id));
            kids.reverse();
            stack.extend(kids);
        }
        // Include any entities orphaned by a missing parent so nothing is lost.
        for e in &self.entities {
            if !out.contains(&e.id) {
                out.push(e.id);
            }
        }
        out
    }
}

pub fn prefab_to_json(entities: &[Entity]) -> Result<String, String> {
    serde_json::to_string_pretty(entities).map_err(|e| format!("failed to serialize prefab: {e}"))
}

pub fn prefab_to_bytes(entities: &[Entity]) -> Result<Vec<u8>, String> {
    encode_binary_document("prefab", PREFAB_BINARY_MAGIC, &entities)
}

pub fn prefab_from_json(text: &str) -> Result<Vec<Entity>, String> {
    let mut entities: Vec<Entity> =
        serde_json::from_str(text).map_err(|e| format!("failed to parse prefab: {e}"))?;
    normalize_entities(&mut entities);
    Ok(entities)
}

pub fn prefab_from_bytes(bytes: &[u8]) -> Result<Vec<Entity>, String> {
    if bytes.starts_with(PREFAB_BINARY_MAGIC) {
        let mut entities: Vec<Entity> =
            decode_binary_document("prefab", PREFAB_BINARY_MAGIC, bytes)?;
        normalize_entities(&mut entities);
        return Ok(entities);
    }

    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("failed to read prefab as UTF-8 JSON: {error}"))?;
    prefab_from_json(text)
}

pub fn load_prefab(path: &Path) -> Result<Vec<Entity>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    prefab_from_bytes(&bytes)
}

pub fn save_prefab(path: &Path, entities: &[Entity]) -> Result<(), String> {
    std::fs::write(path, prefab_to_bytes(entities)?)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn fmt_num(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let mut s = format!("{value:.3}");
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
        s
    }
}

/// Coerce a name into a valid Luau identifier for field assignments.
fn sanitize_field(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_';
        let leading_digit = i == 0 && ch.is_ascii_digit();
        if ok && !leading_digit {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn escape_luau(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

fn normalize_require_path(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    if path.ends_with(".luau") {
        path.truncate(path.len() - ".luau".len());
    } else if path.ends_with(".lua") {
        path.truncate(path.len() - ".lua".len());
    }
    if path.starts_with("./") || path.starts_with("../") || path.starts_with('@') {
        path
    } else {
        format!("./{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let scene = Scene::default();
        let json = scene.to_json().expect("serialize");
        let restored = Scene::from_json(&json).expect("deserialize");
        assert_eq!(restored.entities.len(), scene.entities.len());
        assert_eq!(restored.name, scene.name);
    }

    #[test]
    fn scene_binary_format_round_trips_and_is_smaller_for_editor_documents() {
        let mut scene = Scene::default();
        scene.name = "Compact".into();
        scene.background = [1, 2, 3, 255];
        for index in 0..40 {
            let mut entity = scene.add_entity(format!("Entity {index}"), index as f32, 12.0);
            entity.components.push(Component::core("Rect2D"));
            let id = entity.id;
            scene.replace_entity(id, entity);
        }

        let json = scene.to_json().expect("json");
        let bytes = scene.to_bytes().expect("binary");
        assert!(bytes.starts_with(SCENE_BINARY_MAGIC));
        assert!(
            bytes.len() < json.len(),
            "binary={} json={}",
            bytes.len(),
            json.len()
        );

        let restored = Scene::from_bytes(&bytes).expect("restore binary");
        assert_eq!(restored.name, "Compact");
        assert_eq!(restored.entities.len(), scene.entities.len());
        let json_restored = Scene::from_bytes(json.as_bytes()).expect("restore legacy json");
        assert_eq!(json_restored.entities.len(), scene.entities.len());
    }

    #[test]
    fn prefab_binary_format_round_trips_and_legacy_json_still_loads() {
        let mut root = Entity::new(10, "Root", 1.0, 2.0);
        root.components.push(Component::core("Rect2D"));
        let mut child = Entity::new(11, "Child", 3.0, 4.0);
        child.parent = Some(root.id);
        child.components.push(Component::Script {
            path: "scripts/Child.luau".into(),
            variables: Vec::new(),
        });
        let entities = vec![root, child];

        let json = prefab_to_json(&entities).expect("json");
        let bytes = prefab_to_bytes(&entities).expect("binary");
        assert!(bytes.starts_with(PREFAB_BINARY_MAGIC));

        let restored = prefab_from_bytes(&bytes).expect("restore binary");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].name, "Root");
        assert_eq!(restored[1].parent, Some(10));

        let legacy = prefab_from_bytes(json.as_bytes()).expect("restore json");
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[1].name, "Child");
    }

    #[test]
    fn linked_prefab_refresh_preserves_root_placement() {
        let mut prototype_scene = Scene::default();
        let prototype_root = prototype_scene.entities[0].id;
        prototype_scene
            .entity_mut(prototype_root)
            .expect("prototype root exists")
            .name = "Enemy".into();
        let child = prototype_scene.add_entity("Weapon", 4.0, 5.0).id;
        prototype_scene
            .entity_mut(child)
            .expect("child exists")
            .parent = Some(prototype_root);
        let prototype = prototype_scene.subtree(prototype_root);

        let mut scene = Scene::default();
        scene.entities.clear();
        let root = scene
            .instantiate_linked(prototype.clone(), "prefabs/enemy.neoprefab")
            .expect("instantiate linked prefab");
        scene.entity_mut(root).expect("root exists").x = 320.0;
        scene.entity_mut(root).expect("root exists").y = 180.0;

        let mut edited = prototype;
        edited[0].name = "Strong Enemy".into();
        edited.push(Entity::new(99, "Health Bar", 0.0, -12.0));
        edited.last_mut().expect("edited is non-empty").parent = Some(edited[0].id);
        assert_eq!(
            scene.refresh_prefab_instances("prefabs/enemy.neoprefab", &edited),
            1
        );
        let refreshed = scene.entity(root).expect("root still exists after refresh");
        assert_eq!(refreshed.name, "Strong Enemy");
        assert_eq!((refreshed.x, refreshed.y), (320.0, 180.0));
        assert_eq!(scene.children_of(Some(root)).len(), 2);
    }

    #[test]
    fn default_entity_has_no_components() {
        let scene = Scene::default();
        assert_eq!(scene.entities.len(), 1);
        assert!(scene.entities[0].components.is_empty());
    }

    #[test]
    fn core_component_exports_real_engine_calls() {
        let mut scene = Scene {
            name: "Test".into(),
            background: [10, 20, 30, 255],
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities: Vec::new(),
            next_id: 1,
        };
        let mut e = scene.add_entity("Hero", 50.0, 60.0);
        e.z = 5.0;
        e.components.push(Component::core("TextBox"));
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau();
        assert!(luau.contains("ecs.newEntity(\"Hero\", ecs.root, 50, 60)"));
        assert!(luau.contains(".z = 5"));
        assert!(luau.contains("AddComponent(core.TextBox)"));
        assert!(luau.contains(".text = \"Text\""));
        assert!(luau.contains(".align_x = \"left\""));
        assert!(luau.contains("app.antiAliasing = \"high\""));
        assert!(luau.contains(".antialiasing = \"inherit\""));
    }

    #[test]
    fn particle_system_has_editor_schema_and_exports_runtime_component() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let particle_system = Component::core("ParticleSystem2D");
        let Component::Core { props, .. } = &particle_system else {
            unreachable!()
        };
        assert!(props.iter().any(|prop| prop.name == "emission_rate"));
        assert!(props.iter().any(|prop| prop.name == "color_sequence"));
        assert!(
            props
                .iter()
                .any(|prop| prop.name == "transparency_sequence")
        );
        assert!(
            props
                .iter()
                .any(|prop| prop.name == "gravity_y" && prop.advanced)
        );
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(particle_system);

        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(core.ParticleSystem2D)"));
        assert!(luau.contains(".emission_rate = 12"));
        assert!(
            luau.contains(".color_sequence = {{ time = 0, color = Color4(255, 184, 76, 255) }")
        );
        assert!(luau.contains(".transparency_sequence = {{ time = 0, value = 0 }"));
    }

    #[test]
    fn drawable_shader_paths_export_as_shader_handles() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut rect = Component::core("Rect2D");
        let Component::Core { props, .. } = &mut rect else {
            unreachable!()
        };
        props
            .iter_mut()
            .find(|prop| prop.name == "shader")
            .expect("shader property")
            .value = PropValue::Shader("shaders/glow.glsl".into());
        scene.entity_mut(id).expect("entity").components.push(rect);

        assert!(
            scene
                .to_luau()
                .contains(".shader = shaders.loadFragment(\"shaders/glow.glsl\")")
        );
    }

    #[test]
    fn spatial_sound_has_asset_schema_and_exports_sound_handle() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut sound = Component::core("SpatialSound2D");
        let Component::Core { props, .. } = &mut sound else {
            unreachable!()
        };
        let sound_prop = props
            .iter_mut()
            .find(|prop| prop.name == "sound")
            .expect("sound property");
        assert!(matches!(sound_prop.value, PropValue::Sound(_)));
        sound_prop.value = PropValue::Sound("assets/ambience.ogg".into());
        scene.entity_mut(id).expect("entity").components.push(sound);

        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(core.SpatialSound2D)"));
        assert!(luau.contains(".sound = assets.loadSound(\"assets/ambience.ogg\")"));
        assert!(luau.contains(".volume = 1"));
    }

    #[test]
    fn old_text_font_properties_upgrade_to_font_assets() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut text = Component::core("TextBox");
        let Component::Core { props, .. } = &mut text else {
            unreachable!()
        };
        props
            .iter_mut()
            .find(|prop| prop.name == "font")
            .expect("font property")
            .value = PropValue::Text("assets/legacy.ttf".into());
        scene.entity_mut(id).expect("entity").components.push(text);

        let restored = Scene::from_json(&scene.to_json().expect("serialize")).expect("restore");
        let Component::Core { props, .. } = &restored.entities[0].components[0] else {
            unreachable!()
        };
        assert!(matches!(
            &props.iter().find(|prop| prop.name == "font").expect("font").value,
            PropValue::Font(path) if path == "assets/legacy.ttf"
        ));
    }

    #[test]
    fn old_particle_colors_upgrade_to_lifetime_sequences() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut particle = Component::core("ParticleSystem2D");
        let Component::Core { props, .. } = &mut particle else {
            unreachable!()
        };
        props.retain(|prop| {
            prop.name != "color_sequence"
                && prop.name != "transparency_sequence"
                && prop.name != "shader"
        });
        props.push(Prop::color("start_color", "Start Color", [10, 20, 30, 204]));
        props.push(Prop::color("end_color", "End Color", [40, 50, 60, 51]));
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(particle);

        let restored = Scene::from_json(&scene.to_json().expect("serialize")).expect("restore");
        let Component::Core { props, .. } = &restored.entities[0].components[0] else {
            unreachable!()
        };
        assert!(props.iter().any(|prop| matches!(
            &prop.value,
            PropValue::ColorSequence(keypoints)
                if keypoints[0].color == [10, 20, 30, 255]
                    && keypoints[1].color == [40, 50, 60, 255]
        )));
        assert!(props.iter().any(|prop| matches!(
            &prop.value,
            PropValue::NumberSequence(keypoints)
                if (keypoints[0].value - 0.2).abs() < 0.001
                    && (keypoints[1].value - 0.8).abs() < 0.001
        )));
        assert!(
            props
                .iter()
                .any(|prop| matches!(prop.value, PropValue::Shader(_)))
        );
        assert!(
            !props
                .iter()
                .any(|prop| prop.name == "start_color" || prop.name == "end_color")
        );
    }

    #[test]
    fn tilemap_has_editor_schema_and_exports_runtime_component() {
        let component = Component::core("Tilemap2D");
        let Component::Core { props, .. } = &component else {
            unreachable!()
        };
        assert!(props.iter().any(|prop| prop.name == "tiles"));
        assert!(props.iter().any(|prop| prop.name == "map_width"));
        let mut scene = Scene::default();
        scene.entities[0].components.push(component);
        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(core.Tilemap2D)"));
        assert!(luau.contains(".tiles = \"0\""));
    }

    #[test]
    fn parents_are_emitted_before_children() {
        let mut scene = Scene {
            name: "T".into(),
            background: [0, 0, 0, 255],
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities: Vec::new(),
            next_id: 1,
        };
        let parent = scene.add_entity("Parent", 0.0, 0.0).id;
        let mut child = scene.add_entity("Child", 0.0, 0.0);
        child.parent = Some(parent);
        let cid = child.id;
        scene.replace_entity(cid, child);

        let luau = scene.to_luau();
        let parent_pos = luau.find("\"Parent\"").expect("parent emitted");
        let child_pos = luau.find("\"Child\"").expect("child emitted");
        assert!(parent_pos < child_pos);
        // The child should reference the parent variable, not ecs.root.
        assert!(luau.contains("ecs.newEntity(\"Child\", ent_0"));
    }

    #[test]
    fn entity_pivots_export_to_luau() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        {
            let entity = scene.entity_mut(id).expect("entity");
            entity.position_pivot = "center".to_string();
            entity.pivot_x = Some(0.25);
            entity.pivot_y = Some(0.75);
            entity.rotation_pivot = "center".to_string();
            entity.rotation_pivot_x = Some(0.5);
            entity.rotation_pivot_y = Some(1.0);
        }

        let luau = scene.to_luau();
        assert!(luau.contains(".position_pivot = \"center\""));
        assert!(luau.contains(".pivot_x = 0.25"));
        assert!(luau.contains(".pivot_y = 0.75"));
        assert!(luau.contains(".rotation_pivot = \"center\""));
        assert!(luau.contains(".rotation_pivot_x = 0.5"));
        assert!(luau.contains(".rotation_pivot_y = 1"));
    }

    #[test]
    fn reparent_cycle_detection() {
        let mut scene = Scene::default();
        let a = scene.add_entity("A", 0.0, 0.0).id;
        let mut b = scene.add_entity("B", 0.0, 0.0);
        b.parent = Some(a);
        let bid = b.id;
        scene.replace_entity(bid, b);
        // Making A a child of B would create a cycle.
        assert!(scene.would_cycle(a, bid));
        assert!(!scene.would_cycle(bid, a));
    }

    #[test]
    fn script_component_exports_public_variables() {
        let mut scene = Scene {
            name: "S".into(),
            background: [0, 0, 0, 255],
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities: Vec::new(),
            next_id: 1,
        };
        let mut e = scene.add_entity("Player", 0.0, 0.0);
        e.components.push(Component::Script {
            path: "scripts/Player".into(),
            variables: vec![ScriptVar {
                name: "speed".into(),
                value: VarValue::Number(200.0),
                control: VarControl::Field,
            }],
        });
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau();
        assert!(luau.contains("local ScriptModule_0 = require(\"./scripts/Player\")"));
        assert!(luau.contains("AddComponent(ScriptModule_0)"));
        assert!(luau.contains(".speed = 200"));
        assert!(scene.to_images_luau().is_none());
        assert!(
            luau.find("require(\"./scripts/Player\")")
                .expect("require in output")
                < luau.find("app.bg").expect("app.bg in output")
        );
    }

    #[test]
    fn script_component_exports_forward_entity_and_component_references() {
        let mut scene = Scene::default();
        scene.entities.clear();
        let mut owner = scene.add_entity("Owner", 0.0, 0.0);
        let owner_id = owner.id;
        owner.components.push(Component::Script {
            path: "scripts/Owner.luau".into(),
            variables: Vec::new(),
        });
        scene.replace_entity(owner_id, owner);

        let mut target = scene.add_entity("Target", 0.0, 0.0);
        let target_id = target.id;
        target.components.push(Component::core("Rect2D"));
        scene.replace_entity(target_id, target);

        let Component::Script { variables, .. } =
            &mut scene.entity_mut(owner_id).expect("owner exists").components[0]
        else {
            unreachable!()
        };
        variables.push(ScriptVar {
            name: "target".into(),
            value: VarValue::Entity(Some(target_id)),
            control: VarControl::Field,
        });
        variables.push(ScriptVar {
            name: "renderer".into(),
            value: VarValue::Component(Some(ComponentReference {
                entity: target_id,
                component: 0,
            })),
            control: VarControl::Field,
        });

        let luau = scene.to_luau();
        let target_component = luau
            .find("local ent_1_c0 = ent_1:AddComponent(core.Rect2D)")
            .expect("target component declaration");
        let entity_assignment = luau
            .find("ent_0_c0.target = ent_1")
            .expect("entity reference assignment");
        let component_assignment = luau
            .find("ent_0_c0.renderer = ent_1_c0")
            .expect("component reference assignment");
        assert!(entity_assignment > target_component);
        assert!(component_assignment > target_component);
    }

    #[test]
    fn removing_component_clears_or_shifts_component_references() {
        let mut scene = Scene::default();
        let target_id = scene.entities[0].id;
        scene
            .entity_mut(target_id)
            .expect("target exists")
            .components = vec![
            Component::core("Rect2D"),
            Component::core("Shape2D"),
            Component::Script {
                path: "scripts/Refs.luau".into(),
                variables: vec![
                    ScriptVar {
                        name: "removed".into(),
                        value: VarValue::Component(Some(ComponentReference {
                            entity: target_id,
                            component: 0,
                        })),
                        control: VarControl::Field,
                    },
                    ScriptVar {
                        name: "shifted".into(),
                        value: VarValue::Component(Some(ComponentReference {
                            entity: target_id,
                            component: 1,
                        })),
                        control: VarControl::Field,
                    },
                ],
            },
        ];
        scene
            .entity_mut(target_id)
            .expect("target exists")
            .components
            .remove(0);
        scene.adjust_component_references(target_id, 0);

        let Component::Script { variables, .. } =
            &scene.entity(target_id).expect("target exists").components[1]
        else {
            unreachable!()
        };
        assert!(matches!(variables[0].value, VarValue::Component(None)));
        assert!(matches!(
            variables[1].value,
            VarValue::Component(Some(ComponentReference { component: 0, .. }))
        ));
    }

    #[test]
    fn script_component_preserves_valid_require_prefixes() {
        for (path, required) in [
            ("./scripts/A.luau", "./scripts/A"),
            ("../shared/B.lua", "../shared/B"),
            ("@game/C", "@game/C"),
        ] {
            let mut scene = Scene::default();
            let id = scene.entities[0].id;
            scene
                .entity_mut(id)
                .expect("entity")
                .components
                .push(Component::Script {
                    path: path.into(),
                    variables: Vec::new(),
                });
            let luau = scene.to_luau();
            assert!(luau.contains(&format!("local ScriptModule_0 = require(\"{required}\")")));
            assert!(luau.contains("AddComponent(ScriptModule_0)"));
            assert!(
                luau.find(&format!("require(\"{required}\")"))
                    .expect("require in output")
                    < luau.find("app.bg").expect("app.bg in output")
            );
        }
    }

    #[test]
    fn shared_image_is_loaded_once_and_referenced_everywhere() {
        let mut scene = Scene::default();
        // Two sprites pointing at the same image must share one loadImage call.
        for name in ["A", "B"] {
            let mut e = scene.add_entity(name, 0.0, 0.0);
            let mut sprite = Component::core("Sprite2D");
            if let Component::Core { props, .. } = &mut sprite {
                for prop in props.iter_mut() {
                    if let PropValue::Image(path) = &mut prop.value {
                        *path = "assets/shared.png".into();
                    }
                }
            }
            e.components.push(sprite);
            let id = e.id;
            scene.replace_entity(id, e);
        }

        let images = scene.to_images_luau().expect("images emitted");
        assert_eq!(images.matches("assets.loadImage(").count(), 1);
        assert!(
            images.contains(
                "Images[\"assets/shared.png\"] = assets.loadImage(\"assets/shared.png\")"
            )
        );

        let luau = scene.to_luau();
        // Both entities reference the cached handle, none call loadImage inline.
        assert!(!luau.contains("loadImage"));
        assert!(luau.contains("local Images = require(\"./images\")"));
        assert_eq!(luau.matches("Images[\"assets/shared.png\"]").count(), 2);
    }

    #[test]
    fn runtime_luau_inlines_images_instead_of_requiring_shared_module() {
        // Scenes loaded at runtime via `loadScene` cannot rely on the exported
        // start scene's `./images` module, so their image cache is inlined and
        // self-contained.
        let mut scene = Scene::default();
        let mut e = scene.add_entity("Sprite", 0.0, 0.0);
        let mut sprite = Component::core("Sprite2D");
        if let Component::Core { props, .. } = &mut sprite {
            for prop in props.iter_mut() {
                if let PropValue::Image(path) = &mut prop.value {
                    *path = "assets/only-in-scene-2.png".into();
                }
            }
        }
        e.components.push(sprite);
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau_runtime();
        assert!(!luau.contains("require(\"./images\")"));
        assert!(luau.contains(
            "Images[\"assets/only-in-scene-2.png\"] = assets.loadImage(\"assets/only-in-scene-2.png\")"
        ));
        // The component still reads its handle from the inlined cache table.
        assert!(luau.contains("Images[\"assets/only-in-scene-2.png\"]"));
    }

    #[test]
    fn scene_without_images_emits_no_images_module() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("TextBox"));
        assert!(scene.to_images_luau().is_none());
        assert!(!scene.to_luau().contains("require(\"./images\")"));
    }

    #[test]
    fn script_component_exports_color_list_and_dictionary_variables() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::Script {
                path: "scripts/Inventory.luau".into(),
                variables: vec![
                    ScriptVar {
                        name: "tint".into(),
                        value: VarValue::Color([1, 2, 3, 4]),
                        control: VarControl::Field,
                    },
                    ScriptVar {
                        name: "items".into(),
                        value: VarValue::List(vec![
                            VarValue::Text("key".into()),
                            VarValue::Number(2.0),
                        ]),
                        control: VarControl::Field,
                    },
                    ScriptVar {
                        name: "stats".into(),
                        value: VarValue::Dictionary(vec![DictionaryEntry {
                            key: VarKey::Text("health".into()),
                            value: VarValue::Number(100.0),
                        }]),
                        control: VarControl::Field,
                    },
                ],
            });

        let luau = scene.to_luau();
        assert!(luau.contains(".tint = Color4(1, 2, 3, 4)"));
        assert!(luau.contains(".items = {\"key\", 2}"));
        assert!(luau.contains(".stats = {[\"health\"] = 100}"));
    }

    #[test]
    fn textbox_font_is_optional_in_export() {
        let mut scene = Scene {
            name: "F".into(),
            background: [0, 0, 0, 255],
            nearest_neighbor_scaling: true,
            antialiasing: default_antialiasing(),
            entities: Vec::new(),
            next_id: 1,
        };
        let mut e = scene.add_entity("T", 0.0, 0.0);
        e.components.push(Component::core("TextBox"));
        let id = e.id;
        scene.replace_entity(id, e);
        // Empty font: no `.font =` line.
        assert!(!scene.to_luau().contains(".font ="));
        // Set the font and it appears.
        if let Some(Component::Core { props, .. }) =
            scene.entity_mut(id).expect("e").components.get_mut(0)
        {
            if let Some(p) = props.iter_mut().find(|p| p.name == "font") {
                p.value = PropValue::Text("assets/font.ttf".into());
            }
        }
        assert!(scene.to_luau().contains(".font = \"assets/font.ttf\""));
    }

    #[test]
    fn textbox_defaults_fit_entity_bounds() {
        let component = Component::core("TextBox");
        let Component::Core { props, .. } = component else {
            panic!("expected core component");
        };
        let text_scale = props
            .iter()
            .find(|prop| prop.name == "text_scale")
            .expect("text_scale prop");
        assert!(matches!(
            &text_scale.value,
            PropValue::Enum { value, .. } if value == "fit"
        ));
        let line_spacing = props
            .iter()
            .find(|prop| prop.name == "line_spacing")
            .expect("line spacing prop");
        assert_eq!(line_spacing.value, PropValue::Number(1.0));
    }

    #[test]
    fn entity_scaler_exports_core_component() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(Component::core("EntityScaler"));
        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(core.EntityScaler)"));
        assert!(luau.contains(".edit_with_percent = true"));
        assert!(luau.contains(".x_percent = 0"));
        assert!(luau.contains(".size_x_percent = 0"));
        assert!(luau.contains(".size_y_percent = 0"));
        assert!(luau.contains(".pivot_x = 0"));
    }

    #[test]
    fn old_entity_scalers_gain_percent_editing_fields_on_load() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        let mut scaler = Component::core("EntityScaler");
        let Component::Core { props, .. } = &mut scaler else {
            unreachable!();
        };
        props.retain(|prop| {
            !matches!(
                prop.name.as_str(),
                "edit_with_percent" | "size_x_percent" | "size_y_percent"
            )
        });
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(scaler);

        let restored = Scene::from_json(&scene.to_json().expect("serialize")).expect("load");
        let Component::Core { props, .. } = &restored.entities[0].components[0] else {
            panic!("expected core component");
        };
        assert!(matches!(
            props
                .iter()
                .find(|prop| prop.name == "edit_with_percent")
                .map(|prop| &prop.value),
            Some(PropValue::Bool(true))
        ));
        for name in ["size_x_percent", "size_y_percent"] {
            assert!(matches!(
                props.iter().find(|prop| prop.name == name).map(|prop| &prop.value),
                Some(PropValue::Number(value)) if *value == 0.0
            ));
        }
    }

    #[test]
    fn ui_and_legacy_components_export() {
        for name in [
            "Frame",
            "Panel",
            "Button",
            "Slider",
            "TextInput",
            "Dropdown",
            "ScrollList",
            "LegacyBolt2D",
            "String2D",
        ] {
            let mut scene = Scene::default();
            let id = scene.entities[0].id;
            scene
                .entity_mut(id)
                .expect("e")
                .components
                .push(Component::core(name));
            let luau = scene.to_luau();
            assert!(
                luau.contains(&format!("AddComponent(core.{name})")),
                "missing {name}"
            );
        }
    }

    #[test]
    fn normalize_merges_new_ui_state_colours_into_existing_components() {
        // A Button saved before hover/state colours existed: only the old field
        // set is present.
        let mut component = Component::Core {
            name: "Button".to_string(),
            props: vec![
                Prop::text("text", "Text", "Connect"),
                Prop::color("background_color", "Background", [131, 131, 131, 255]),
            ],
        };
        normalize_core_component(&mut component);
        let Component::Core { props, .. } = &component else {
            panic!("expected core component");
        };
        // The user's authored value is preserved.
        let bg = props
            .iter()
            .find(|prop| prop.name == "background_color")
            .expect("background preserved");
        assert!(matches!(bg.value, PropValue::Color([131, 131, 131, 255])));
        // The new hover colour is merged in.
        assert!(
            props.iter().any(|prop| prop.name == "hover_background_color"),
            "hover_background_color should be added"
        );
    }

    #[test]
    fn sanitizes_invalid_field_names() {
        assert_eq!(sanitize_field("max speed"), "max_speed");
        assert_eq!(sanitize_field("2cool"), "_cool");
        assert_eq!(sanitize_field(""), "_");
    }
}
