//! Scene data model for the visual editor.
//!
//! A [`Scene`] is a flat list of [`Entity`] nodes, each parented to the engine
//! root. Scenes are persisted as JSON (`*.neoscene`) and can be exported to a
//! runnable `main.luau` that drives the NeoLOVE runtime.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// An RGBA color stored as four bytes in `[r, g, b, a]` order.
pub type Color = [u8; 4];

/// A typed value for a script's public variable. Mirrors the handful of types
/// that are practical to edit in an inspector, much like Unity's serialized
/// fields.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum VarValue {
    Number(f32),
    Bool(bool),
    Text(String),
    Color(Color),
}

impl VarValue {
    /// A short label for the type, shown in the variable's type menu.
    pub fn type_label(&self) -> &'static str {
        match self {
            VarValue::Number(_) => "Number",
            VarValue::Bool(_) => "Bool",
            VarValue::Text(_) => "Text",
            VarValue::Color(_) => "Color",
        }
    }

    /// Render the value as a Luau literal for export.
    pub fn to_luau(&self) -> String {
        match self {
            VarValue::Number(n) => fmt_num(*n),
            VarValue::Bool(b) => b.to_string(),
            VarValue::Text(s) => format!("\"{}\"", escape_luau(s)),
            VarValue::Color([r, g, b, _]) => format!("Color4({r}, {g}, {b})"),
        }
    }
}

/// A named public variable exposed by a [`Component::Script`], editable in the
/// inspector and written to Luau on export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScriptVar {
    pub name: String,
    pub value: VarValue,
}

/// A component attached to an entity. The tagged representation keeps the JSON
/// readable and lets the editor grow new component kinds without breaking old
/// scene files.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum Component {
    /// A filled rectangle the size of the owning entity.
    Rect2D { color: Color },
    /// A text label drawn at the entity origin.
    Text2D {
        text: String,
        color: Color,
        size: f32,
    },
    /// A sprite that draws an image asset, sized to the entity bounds.
    Sprite { image: String, tint: Color },
    /// A reference to a Luau component module plus the public variables to set
    /// on it — the editor's equivalent of attaching a Unity script with
    /// inspector-exposed fields.
    Script {
        path: String,
        variables: Vec<ScriptVar>,
    },
}

impl Component {
    /// A short human-readable label for the inspector and component menu.
    pub fn label(&self) -> &'static str {
        match self {
            Component::Rect2D { .. } => "Rect2D",
            Component::Text2D { .. } => "Text2D",
            Component::Sprite { .. } => "Sprite",
            Component::Script { .. } => "Script",
        }
    }
}

/// A single object in the scene. Every entity is a child of the engine root and
/// owns a 2D transform plus any number of components.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: u64,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub size_x: f32,
    pub size_y: f32,
    pub rotation: f32,
    pub components: Vec<Component>,
}

impl Entity {
    pub fn new(id: u64, name: impl Into<String>, x: f32, y: f32) -> Self {
        Self {
            id,
            name: name.into(),
            x,
            y,
            size_x: 100.0,
            size_y: 100.0,
            rotation: 0.0,
            components: Vec::new(),
        }
    }
}

/// The complete editable document: a named scene with a background color and an
/// ordered list of entities.
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
        // Seed a starter entity so a fresh scene shows something on screen.
        let mut box_entity = scene.add_entity("Box", 200.0, 150.0);
        box_entity.size_x = 160.0;
        box_entity.size_y = 90.0;
        box_entity.components.push(Component::Rect2D {
            color: [80, 140, 255, 255],
        });
        let id = box_entity.id;
        scene.replace_entity(id, box_entity);
        scene
    }
}

impl Scene {
    /// Create and append a new entity, returning a clone the caller can mutate
    /// before storing it back with [`Scene::replace_entity`].
    pub fn add_entity(&mut self, name: impl Into<String>, x: f32, y: f32) -> Entity {
        let id = self.allocate_id();
        let entity = Entity::new(id, name, x, y);
        self.entities.push(entity.clone());
        entity
    }

    /// Allocate a unique entity id, keeping `next_id` ahead of any loaded ids.
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

    pub fn remove_entity(&mut self, id: u64) {
        self.entities.retain(|e| e.id != id);
    }

    /// Serialize the scene to pretty-printed JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("failed to serialize scene: {e}"))
    }

    /// Parse a scene from JSON, recovering the id allocator.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut scene: Scene =
            serde_json::from_str(text).map_err(|e| format!("failed to parse scene: {e}"))?;
        scene.next_id = scene.entities.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        Ok(scene)
    }

    /// Read a scene file from disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        Self::from_json(&text)
    }

    /// Write the scene to disk as JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_json()?)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Generate a runnable `main.luau` that reconstructs this scene with the
    /// NeoLOVE runtime API.
    pub fn to_luau(&self) -> String {
        let mut out = String::new();
        out.push_str("-- Generated by the NeoLOVE visual editor. Edits may be overwritten.\n");
        out.push_str(&format!("-- Scene: {}\n\n", self.name));
        let [br, bg, bb, _] = self.background;
        out.push_str(&format!("app.bg = Color4({br}, {bg}, {bb})\n\n"));

        for (index, entity) in self.entities.iter().enumerate() {
            let var = format!("ent_{index}");
            let name = escape_luau(&entity.name);
            out.push_str(&format!(
                "local {var} = ecs.newEntity(\"{name}\", ecs.root, {}, {})\n",
                fmt_num(entity.x),
                fmt_num(entity.y),
            ));
            out.push_str(&format!("{var}.size_x = {}\n", fmt_num(entity.size_x)));
            out.push_str(&format!("{var}.size_y = {}\n", fmt_num(entity.size_y)));
            if entity.rotation.abs() > f32::EPSILON {
                out.push_str(&format!("{var}.rotation = {}\n", fmt_num(entity.rotation)));
            }

            for (comp_index, component) in entity.components.iter().enumerate() {
                let comp_var = format!("{var}_c{comp_index}");
                match component {
                    Component::Rect2D { color } => {
                        let [r, g, b, _] = color;
                        out.push_str(&format!(
                            "local {comp_var} = {var}:AddComponent(core.Rect2D)\n"
                        ));
                        out.push_str(&format!("{comp_var}.color = Color4({r}, {g}, {b})\n"));
                    }
                    Component::Text2D { text, color, size } => {
                        let [r, g, b, _] = color;
                        out.push_str(&format!(
                            "local {comp_var} = {var}:AddComponent(core.Text2D)\n"
                        ));
                        out.push_str(&format!("{comp_var}.text = \"{}\"\n", escape_luau(text)));
                        out.push_str(&format!("{comp_var}.color = Color4({r}, {g}, {b})\n"));
                        out.push_str(&format!("{comp_var}.size = {}\n", fmt_num(*size)));
                    }
                    Component::Sprite { image, tint } => {
                        let [r, g, b, _] = tint;
                        out.push_str(&format!(
                            "local {comp_var} = {var}:AddComponent(core.Sprite)\n"
                        ));
                        out.push_str(&format!(
                            "{comp_var}.image = \"{}\"\n",
                            escape_luau(image)
                        ));
                        out.push_str(&format!("{comp_var}.tint = Color4({r}, {g}, {b})\n"));
                    }
                    Component::Script { path, variables } => {
                        // Attach a user-authored Luau component module and set
                        // each public variable, mirroring Unity's inspector.
                        let module = if path.is_empty() {
                            "-- TODO: set script path".to_string()
                        } else {
                            format!("require(\"{}\")", escape_luau(path))
                        };
                        out.push_str(&format!(
                            "local {comp_var} = {var}:AddComponent({module})\n"
                        ));
                        for variable in variables {
                            if variable.name.is_empty() {
                                continue;
                            }
                            out.push_str(&format!(
                                "{comp_var}.{} = {}\n",
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
}

/// Format a float without a trailing `.0` for whole numbers, keeping the
/// generated Luau tidy.
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

/// Coerce a variable name into a valid Luau identifier so generated field
/// assignments always parse. Invalid characters become underscores.
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

/// Escape a string for embedding inside a double-quoted Luau literal.
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
    fn allocates_unique_ids_after_load() {
        let mut scene = Scene::default();
        let a = scene.add_entity("A", 0.0, 0.0).id;
        let b = scene.add_entity("B", 0.0, 0.0).id;
        assert_ne!(a, b);
        let json = scene.to_json().expect("serialize");
        let mut restored = Scene::from_json(&json).expect("deserialize");
        let c = restored.add_entity("C", 0.0, 0.0).id;
        assert!(c > a && c > b);
    }

    #[test]
    fn luau_export_contains_entities_and_components() {
        let mut scene = Scene {
            name: "Test".into(),
            background: [10, 20, 30, 255],
            entities: Vec::new(),
            next_id: 1,
        };
        let mut e = scene.add_entity("Hero", 50.0, 60.0);
        e.size_x = 32.0;
        e.components.push(Component::Rect2D {
            color: [255, 0, 0, 255],
        });
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau();
        assert!(luau.contains("app.bg = Color4(10, 20, 30)"));
        assert!(luau.contains("ecs.newEntity(\"Hero\", ecs.root, 50, 60)"));
        assert!(luau.contains("AddComponent(core.Rect2D)"));
        assert!(luau.contains("Color4(255, 0, 0)"));
    }

    #[test]
    fn escapes_luau_strings() {
        assert_eq!(escape_luau("a\"b\\c"), "a\\\"b\\\\c");
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
            variables: vec![
                ScriptVar {
                    name: "speed".into(),
                    value: VarValue::Number(200.0),
                },
                ScriptVar {
                    name: "enabled".into(),
                    value: VarValue::Bool(true),
                },
                ScriptVar {
                    name: "title".into(),
                    value: VarValue::Text("Hi".into()),
                },
            ],
        });
        let id = e.id;
        scene.replace_entity(id, e);

        let luau = scene.to_luau();
        assert!(luau.contains("AddComponent(require(\"scripts/Player\"))"));
        assert!(luau.contains(".speed = 200"));
        assert!(luau.contains(".enabled = true"));
        assert!(luau.contains(".title = \"Hi\""));
    }

    #[test]
    fn sanitizes_invalid_field_names() {
        assert_eq!(sanitize_field("max speed"), "max_speed");
        assert_eq!(sanitize_field("2cool"), "_cool");
        assert_eq!(sanitize_field(""), "_");
    }
}
