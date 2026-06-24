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
    fn new(name: &str, label: &str, value: PropValue, advanced: bool) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            value,
            advanced,
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
}

impl VarValue {
    pub fn type_label(&self) -> &'static str {
        match self {
            VarValue::Number(_) => "Number",
            VarValue::Bool(_) => "Bool",
            VarValue::Text(_) => "Text",
            VarValue::Color(_) => "Color",
        }
    }

    pub fn to_luau(&self) -> String {
        match self {
            VarValue::Number(n) => fmt_num(*n),
            VarValue::Bool(b) => b.to_string(),
            VarValue::Text(s) => format!("\"{}\"", escape_luau(s)),
            VarValue::Color([r, g, b, _]) => format!("Color4({r}, {g}, {b})"),
        }
    }
}

/// A named public variable exposed by a script component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScriptVar {
    pub name: String,
    pub value: VarValue,
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

/// The list of core components offered in the "Add Component" menu, in display
/// order, matching the engine's `core` module.
pub const CORE_COMPONENTS: &[&str] = &[
    "Rect2D",
    "Shape2D",
    "TextBox",
    "Sprite2D",
    "NineSliceSprite2D",
    "TileTexture2D",
    "Collider2D",
    "Rigidbody2D",
    "Bolt2D",
    "Rope2D",
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
        "TextBox" => {
            let mut p = vec![
                Prop::text("text", "Text", "Text"),
                Prop::color("color", "Color", [255, 255, 255, 255]),
                Prop::boolean("visible", "Visible", true),
                Prop::num("scale", "Scale", 24.0),
                Prop::enumv("align_x", "Align X", "left", &["left", "center", "right"], false),
                Prop::enumv("align_y", "Align Y", "top", &["top", "center", "bottom"], false),
                Prop::enumv("wrap", "Wrap", "none", &["none", "word", "char"], false),
            ];
            p.push(Prop::num_adv("min_scale", "Min Scale", 1.0));
            p.push(Prop::enumv(
                "text_scale",
                "Text Scale",
                "none",
                &["none", "fit", "fit_width", "fit_height"],
                true,
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
            p.push(Prop::num_adv("line_spacing", "Line Space", 0.0));
            p.push(Prop::num_adv("letter_spacing", "Letter Space", 0.0));
            p.push(Prop::num_adv("tab_size", "Tab Size", 4.0));
            p
        }
        "Sprite2D" => {
            let mut p = vec![
                Prop::text("image", "Image", "assets/sprite.png"),
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
                Prop::text("image", "Image", "assets/sprite.png"),
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
            Prop::text("image", "Image", "assets/tile.png"),
            Prop::color("color", "Tint", [255, 255, 255, 255]),
            Prop::boolean("visible", "Visible", true),
            Prop::num("tile_width", "Tile W", 32.0),
            Prop::num("tile_height", "Tile H", 32.0),
            Prop::num_adv("offset_x", "Offset X", 0.0),
            Prop::num_adv("offset_y", "Offset Y", 0.0),
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
        "Rope2D" => vec![
            Prop::boolean("enabled", "Enabled", true),
            Prop::num("min_length", "Min Length", 0.0),
            Prop::num("max_length", "Max Length", 100.0),
            Prop::num("stiffness", "Stiffness", 1.0),
            Prop::num("damping", "Damping", 0.0),
            Prop::num_adv("break_force", "Break Force", 0.0),
        ],
        _ => Vec::new(),
    }
}

/// A single object in the scene.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: u64,
    pub name: String,
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
    pub entities: Vec<Entity>,
    #[serde(skip)]
    next_id: u64,
}

impl Default for Scene {
    fn default() -> Self {
        let mut scene = Self {
            name: "Untitled".to_string(),
            background: [24, 26, 32, 255],
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
        let [br, bg, bb, _] = self.background;
        out.push_str(&format!("app.bg = Color4({br}, {bg}, {bb})\n\n"));

        // Emit parents before children so `ecs.newEntity(..., parentVar)` works.
        let ordered = self.topological_order();
        let mut var_of = std::collections::HashMap::new();
        for (index, id) in ordered.iter().enumerate() {
            let Some(entity) = self.entity(*id) else {
                continue;
            };
            // Skip inactive entities and anything beneath an inactive ancestor.
            if !self.is_active_in_tree(*id) {
                continue;
            }
            let var = format!("ent_{index}");
            var_of.insert(*id, var.clone());
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
                            format!("require(\"{}\")", escape_luau(path))
                        };
                        out.push_str(&format!("local {cvar} = {var}:AddComponent({module})\n"));
                        for variable in variables {
                            if variable.name.is_empty() {
                                continue;
                            }
                            out.push_str(&format!(
                                "{cvar}.{} = {}\n",
                                sanitize_field(&variable.name),
                                variable.value.to_luau()
                            ));
                        }
                    }
                }
            }
            out.push('\n');
        }
        out
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
    }

    #[test]
    fn parents_are_emitted_before_children() {
        let mut scene = Scene {
            name: "T".into(),
            background: [0, 0, 0, 255],
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
            entities: Vec::new(),
            next_id: 1,
        };
        let mut e = scene.add_entity("Player", 0.0, 0.0);
        e.components.push(Component::Script {
            path: "scripts/Player".into(),
            variables: vec![ScriptVar {
                name: "speed".into(),
                value: VarValue::Number(200.0),
            }],
        });
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(require(\"scripts/Player\"))"));
        assert!(luau.contains(".speed = 200"));
    }

    #[test]
    fn sanitizes_invalid_field_names() {
        assert_eq!(sanitize_field("max speed"), "max_speed");
        assert_eq!(sanitize_field("2cool"), "_cool");
        assert_eq!(sanitize_field(""), "_");
    }
}
