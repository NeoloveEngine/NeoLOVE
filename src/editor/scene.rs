//! Scene data model for the visual editor.
//!
//! A [`Scene`] is a list of [`Entity`] nodes. Each entity owns a 2D transform
//! and any number of [`Component`]s. Components are data-driven: a built-in
//! ("core") component is just a kind name plus a list of typed [`Prop`]s that
//! mirror the real engine `core.*` components, so the inspector and the Luau
//! exporter stay in lockstep with the runtime. Scenes are persisted as JSON
//! (`*.neoscene`) and can be exported to a runnable `main.luau`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// An RGBA color stored as four bytes in `[r, g, b, a]` order.
pub type Color = [u8; 4];

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
    Enum { value: String, options: Vec<String> },
    /// An image asset path. Exported as `assets.loadImage("...")` so the
    /// runtime receives an ImageHandle rather than a bare string.
    Image(String),
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
    /// An optional text prop (omitted from export when empty), e.g. a font path.
    fn opt_text(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value: PropValue::Text(String::new()),
            advanced: false,
            optional: true,
        }
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
            VarValue::List(values) => format!(
                "{{{}}}",
                values.iter().map(Self::to_luau).collect::<Vec<_>>().join(", ")
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
            Self::Dictionary(entries) => entries
                .iter()
                .any(|entry| entry.value.contains_reference()),
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
                if reference.as_ref().is_some_and(|reference| reference.entity == id) =>
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
    if name != "EntityScaler" {
        return;
    }

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

/// Common core components offered directly in the "Add Component" menu, in
/// display order, matching the engine's `core` module.
pub const CORE_COMPONENTS: &[&str] = &[
    "Rect2D",
    "Shape2D",
    "ParticleSystem2D",
    "TextBox",
    "TextLabel",
    "Sprite2D",
    "Image2D",
    "NineSliceSprite2D",
    "Tilemap2D",
    "TileTexture2D",
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
            Prop::color("start_color", "Start Color", [255, 184, 76, 255]),
            Prop::color("end_color", "End Color", [255, 92, 40, 0]),
            Prop::enumv("shape", "Emitter", "point", &["point", "box", "circle"], false),
            Prop::num("radius", "Radius", 32.0),
            Prop::num_adv("gravity_x", "Gravity X", 0.0),
            Prop::num_adv("gravity_y", "Gravity Y", 60.0),
        ],
        "TextBox" | "TextLabel" | "RudimentaryTextLabel" => {
            let mut p = vec![
                Prop::text("text", "Text", "Text"),
                Prop::color("color", "Color", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
                Prop::num("scale", "Scale", 24.0),
                Prop::enumv(
                    "antialiasing",
                    "Anti-aliasing",
                    "inherit",
                    &["inherit", "off", "standard", "high"],
                    false,
                ),
                Prop::opt_text("font", "Font"),
                Prop::enumv("align_x", "Align X", "left", &["left", "center", "right"], false),
                Prop::enumv("align_y", "Align Y", "top", &["top", "center", "bottom"], false),
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
            ];
            p.push(Prop::num_adv("source_x", "Source X", 0.0));
            p.push(Prop::num_adv("source_y", "Source Y", 0.0));
            p.push(Prop::num_adv("source_w", "Source W", 0.0));
            p.push(Prop::num_adv("source_h", "Source H", 0.0));
            p
        }
        "NineSliceSprite2D" => {
            let mut p = vec![
                Prop::image("image", "Image", "assets/sprite.png"),
                Prop::color("color", "Tint", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
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
            Prop::num("tile_width", "Tile W", 32.0),
            Prop::num("tile_height", "Tile H", 32.0),
            Prop::num_adv("offset_x", "Offset X", 0.0),
            Prop::num_adv("offset_y", "Offset Y", 0.0),
        ],
        "Tilemap2D" => vec![
            Prop::image("image", "Tileset", "assets/tiles.png"),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
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
            Prop::enumv("shape", "Shape", "box", &["box", "circle", "triangle"], false),
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
        "Frame" => vec![
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("corner_radius", "Corner", 10.0),
            Prop::num("padding", "Padding", 8.0),
        ],
        "Button" => vec![
            Prop::text("text", "Text", "Button"),
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("scale", "Scale", 18.0),
            Prop::enumv("align_x", "Align X", "center", &["left", "center", "right"], false),
            Prop::num("corner_radius", "Corner", 8.0),
            Prop::num_adv("padding_x", "Padding X", 12.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
            Prop::num_adv("icon_gap", "Icon Gap", 8.0),
        ],
        "TextInput" => vec![
            Prop::text("text", "Text", ""),
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("scale", "Scale", 18.0),
            Prop::enumv("align_x", "Align X", "left", &["left", "center", "right"], false),
            Prop::num("corner_radius", "Corner", 8.0),
            Prop::int("max_length", "Max Length", 0),
            Prop::boolean("password", "Password", false),
            Prop::boolean_adv("submit_on_enter", "Submit Enter", true),
            Prop::boolean_adv("clear_on_submit", "Clear Submit", false),
            Prop::num_adv("border_width", "Border", 1.0),
            Prop::num_adv("padding_x", "Padding X", 10.0),
            Prop::num_adv("padding_y", "Padding Y", 8.0),
        ],
        "Dropdown" => vec![
            Prop::color("color", "Color", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("scale", "Scale", 18.0),
            Prop::num("item_height", "Item H", 32.0),
            Prop::int("max_visible_items", "Max Items", 8),
            Prop::num("corner_radius", "Corner", 8.0),
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
            background: [24, 26, 32, 255],
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
            background: [24, 26, 32, 255],
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
    pub fn instantiate_linked(&mut self, proto: Vec<Entity>, source: impl Into<String>) -> Option<u64> {
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
            let Some(instance_root) = self.entity(root_id).cloned() else { continue; };
            let removed: std::collections::HashSet<u64> =
                self.subtree(root_id).into_iter().map(|entity| entity.id).collect();
            self.entities.retain(|entity| !removed.contains(&entity.id));

            let mut id_map = std::collections::HashMap::new();
            id_map.insert(proto_root.id, root_id);
            for entity in proto.iter().filter(|entity| entity.id != proto_root.id) {
                id_map.insert(entity.id, self.allocate_id());
            }
            for mut entity in proto.iter().cloned() {
                let old_id = entity.id;
                entity.id = id_map[&old_id];
                entity.parent = entity.parent.and_then(|parent| id_map.get(&parent).copied());
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

    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut scene: Scene =
            serde_json::from_str(text).map_err(|e| format!("failed to parse scene: {e}"))?;
        for entity in &mut scene.entities {
            for component in &mut entity.components {
                normalize_core_component(component);
            }
        }
        scene.next_id = scene.entities.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        Ok(scene)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        Self::from_json(&text)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_json()?)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Generate a runnable `main.luau` reconstructing this scene.
    pub fn to_luau(&self) -> String {
        let mut out = String::new();
        out.push_str("-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\n");
        out.push_str(&format!("-- Scene: {}\n\n", self.name));

        // Require code modules at the top of main.luau. Images remain in their
        // own generated cache module because loading/retaining image handles is
        // asset work, while component code belongs in the entry module.
        let (image_paths, script_paths) = self.collect_assets();
        if !image_paths.is_empty() {
            out.push_str("local Images = require(\"./images\")\n");
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

            for (ci, component) in entity.components.iter().enumerate() {
                let cvar = format!("{var}_c{ci}");
                match component {
                    Component::Core { name, props } => {
                        out.push_str(&format!(
                            "local {cvar} = {var}:AddComponent(core.{name})\n"
                        ));
                        for prop in props {
                            // Skip optional text props left empty, and empty
                            // image paths, so the runtime keeps its default.
                            if prop.optional {
                                if let PropValue::Text(t) = &prop.value {
                                    if t.is_empty() {
                                        continue;
                                    }
                                }
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
    fn linked_prefab_refresh_preserves_root_placement() {
        let mut prototype_scene = Scene::default();
        let prototype_root = prototype_scene.entities[0].id;
        prototype_scene.entity_mut(prototype_root).expect("prototype root exists").name = "Enemy".into();
        let child = prototype_scene.add_entity("Weapon", 4.0, 5.0).id;
        prototype_scene.entity_mut(child).expect("child exists").parent = Some(prototype_root);
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
        assert!(props.iter().any(|prop| prop.name == "start_color"));
        assert!(props.iter().any(|prop| prop.name == "gravity_y" && prop.advanced));
        scene
            .entity_mut(id)
            .expect("entity")
            .components
            .push(particle_system);

        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(core.ParticleSystem2D)"));
        assert!(luau.contains(".emission_rate = 12"));
        assert!(luau.contains(".end_color = Color4(255, 92, 40, 0)"));
    }

    #[test]
    fn tilemap_has_editor_schema_and_exports_runtime_component() {
        let component = Component::core("Tilemap2D");
        let Component::Core { props, .. } = &component else { unreachable!() };
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
        assert!(luau.find("require(\"./scripts/Player\")").expect("require in output") < luau.find("app.bg").expect("app.bg in output"));
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
        scene.entity_mut(target_id).expect("target exists").components = vec![
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
        scene.entity_mut(target_id).expect("target exists").components.remove(0);
        scene.adjust_component_references(target_id, 0);

        let Component::Script { variables, .. } = &scene.entity(target_id).expect("target exists").components[1]
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
            scene.entity_mut(id).expect("entity").components.push(Component::Script {
                path: path.into(),
                variables: Vec::new(),
            });
            let luau = scene.to_luau();
            assert!(luau.contains(&format!("local ScriptModule_0 = require(\"{required}\")")));
            assert!(luau.contains("AddComponent(ScriptModule_0)"));
            assert!(luau.find(&format!("require(\"{required}\")")).expect("require in output") < luau.find("app.bg").expect("app.bg in output"));
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
        assert!(images.contains("Images[\"assets/shared.png\"] = assets.loadImage(\"assets/shared.png\")"));

        let luau = scene.to_luau();
        // Both entities reference the cached handle, none call loadImage inline.
        assert!(!luau.contains("loadImage"));
        assert!(luau.contains("local Images = require(\"./images\")"));
        assert_eq!(luau.matches("Images[\"assets/shared.png\"]").count(), 2);
    }

    #[test]
    fn scene_without_images_emits_no_images_module() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene.entity_mut(id).expect("entity").components.push(Component::core("TextBox"));
        assert!(scene.to_images_luau().is_none());
        assert!(!scene.to_luau().contains("require(\"./images\")"));
    }

    #[test]
    fn script_component_exports_color_list_and_dictionary_variables() {
        let mut scene = Scene::default();
        let id = scene.entities[0].id;
        scene.entity_mut(id).expect("entity").components.push(Component::Script {
            path: "scripts/Inventory.luau".into(),
            variables: vec![
                ScriptVar {
                    name: "tint".into(),
                    value: VarValue::Color([1, 2, 3, 4]),
                    control: VarControl::Field,
                },
                ScriptVar {
                    name: "items".into(),
                    value: VarValue::List(vec![VarValue::Text("key".into()), VarValue::Number(2.0)]),
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
        scene.entity_mut(id).expect("entity").components.push(scaler);

        let restored = Scene::from_json(&scene.to_json().expect("serialize")).expect("load");
        let Component::Core { props, .. } = &restored.entities[0].components[0] else {
            panic!("expected core component");
        };
        assert!(matches!(
            props.iter().find(|prop| prop.name == "edit_with_percent").map(|prop| &prop.value),
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
        for name in ["Frame", "Button", "TextInput", "Dropdown", "ScrollList", "LegacyBolt2D", "String2D"] {
            let mut scene = Scene::default();
            let id = scene.entities[0].id;
            scene.entity_mut(id).expect("e").components.push(Component::core(name));
            let luau = scene.to_luau();
            assert!(luau.contains(&format!("AddComponent(core.{name})")), "missing {name}");
        }
    }

    #[test]
    fn sanitizes_invalid_field_names() {
        assert_eq!(sanitize_field("max speed"), "max_speed");
        assert_eq!(sanitize_field("2cool"), "_cool");
        assert_eq!(sanitize_field(""), "_");
    }
}
