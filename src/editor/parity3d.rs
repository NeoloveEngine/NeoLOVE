//! Structured authored-scene versus real-runtime parity checks for 3D play.
//!
//! The validator intentionally consumes the runtime's immutable post-load,
//! pre-update snapshot. Comparing against a later live snapshot would report
//! intentional script, animation, and physics changes as serialization bugs.

use std::collections::{BTreeMap, HashMap};

use crate::scene::{Component, Prop, PropValue, Scene, SceneKind, VarValue};
use crate::window::{ComponentSnapshot, EntitySnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ParityCategory {
    Serialization,
    Hierarchy,
    Transform,
    Component,
    Mesh,
    Material,
    Texture,
    Shader,
    Lighting,
    Shadowing,
    Environment,
    Camera,
    Physics,
    Animation,
    Particle,
    Script,
}

impl ParityCategory {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Serialization => "SERIALIZATION",
            Self::Hierarchy => "HIERARCHY",
            Self::Transform => "TRANSFORM",
            Self::Component => "COMPONENT",
            Self::Mesh => "MESH",
            Self::Material => "MATERIAL",
            Self::Texture => "TEXTURE",
            Self::Shader => "SHADER",
            Self::Lighting => "LIGHTING",
            Self::Shadowing => "SHADOWING",
            Self::Environment => "ENVIRONMENT",
            Self::Camera => "CAMERA",
            Self::Physics => "PHYSICS",
            Self::Animation => "ANIMATION",
            Self::Particle => "PARTICLE",
            Self::Script => "SCRIPT",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParityMismatch {
    pub category: ParityCategory,
    pub entity_id: Option<u64>,
    pub component_index: Option<usize>,
    pub component: Option<String>,
    pub property: Option<String>,
    pub expected: String,
    pub actual: String,
}

impl ParityMismatch {
    pub(crate) fn context(&self) -> String {
        let mut context = self
            .entity_id
            .map(|id| format!("entity #{id}"))
            .unwrap_or_else(|| "runtime scene".to_string());
        if let Some(component) = &self.component {
            context.push_str(" · ");
            context.push_str(component);
            if let Some(index) = self.component_index {
                context.push_str(&format!(" [{index}]"));
            }
        }
        if let Some(property) = &self.property {
            context.push_str(" · ");
            context.push_str(property);
        }
        context
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParityReport {
    pub authored_entities: usize,
    pub runtime_entities: usize,
    pub mismatches: Vec<ParityMismatch>,
}

impl ParityReport {
    pub(crate) fn is_match(&self) -> bool {
        self.mismatches.is_empty()
    }

    pub(crate) fn category_counts(&self) -> BTreeMap<ParityCategory, usize> {
        let mut counts = BTreeMap::new();
        for mismatch in &self.mismatches {
            *counts.entry(mismatch.category).or_default() += 1;
        }
        counts
    }
}

fn mismatch(
    report: &mut ParityReport,
    category: ParityCategory,
    entity_id: Option<u64>,
    component_index: Option<usize>,
    component: Option<&str>,
    property: Option<&str>,
    expected: impl Into<String>,
    actual: impl Into<String>,
) {
    report.mismatches.push(ParityMismatch {
        category,
        entity_id,
        component_index,
        component: component.map(str::to_string),
        property: property.map(str::to_string),
        expected: expected.into(),
        actual: actual.into(),
    });
}

fn fields(snapshot: &EntitySnapshot) -> HashMap<&str, &str> {
    snapshot
        .fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn component_fields(snapshot: &ComponentSnapshot) -> HashMap<&str, &str> {
    snapshot
        .fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn number_equal(expected: f32, actual: &str) -> bool {
    let Ok(actual) = actual.parse::<f32>() else {
        return false;
    };
    actual.is_finite()
        && (expected - actual).abs() <= 1.0e-4 * expected.abs().max(actual.abs()).max(1.0)
}

fn compare_number(
    report: &mut ParityReport,
    entity_id: u64,
    property: &str,
    expected: f32,
    actual: Option<&str>,
) {
    if actual.is_some_and(|actual| number_equal(expected, actual)) {
        return;
    }
    mismatch(
        report,
        ParityCategory::Transform,
        Some(entity_id),
        None,
        None,
        Some(property),
        expected.to_string(),
        actual.unwrap_or("<missing>"),
    );
}

fn compare_number_value(
    report: &mut ParityReport,
    entity_id: u64,
    property: &str,
    expected: f32,
    actual: f32,
) {
    if actual.is_finite()
        && (expected - actual).abs() <= 1.0e-4 * expected.abs().max(actual.abs()).max(1.0)
    {
        return;
    }
    mismatch(
        report,
        ParityCategory::Transform,
        Some(entity_id),
        None,
        None,
        Some(property),
        expected.to_string(),
        actual.to_string(),
    );
}

fn authored_component_key(component: &Component) -> String {
    match component {
        Component::Core { name, .. } => format!("core:{name}"),
        Component::Script { path, .. } => format!("script:{path}"),
    }
}

fn component_category(component: &Component) -> ParityCategory {
    match component {
        Component::Script { .. } => ParityCategory::Script,
        Component::Core { name, .. } if name.contains("Camera") => ParityCategory::Camera,
        Component::Core { name, .. }
            if name.contains("Environment") || name.contains("Skybox") =>
        {
            ParityCategory::Environment
        }
        Component::Core { name, .. } if name.contains("Light") => ParityCategory::Lighting,
        Component::Core { name, .. }
            if name.contains("Rigid")
                || name.contains("Collider")
                || name.contains("Trigger")
                || name.contains("CharacterController") =>
        {
            ParityCategory::Physics
        }
        Component::Core { name, .. }
            if name.contains("Animation") || name.contains("Animator") =>
        {
            ParityCategory::Animation
        }
        Component::Core { name, .. } if name.contains("Particle") => ParityCategory::Particle,
        Component::Core { name, .. } if name.contains("Mesh") || name.contains("LOD") => {
            ParityCategory::Mesh
        }
        Component::Core { .. } => ParityCategory::Component,
    }
}

fn property_category(component: &Component, prop: &Prop) -> ParityCategory {
    if prop.name.to_ascii_lowercase().contains("shadow") {
        return ParityCategory::Shadowing;
    }
    match &prop.value {
        PropValue::Material(_) => ParityCategory::Material,
        PropValue::PhysicsMaterial(_) => ParityCategory::Physics,
        PropValue::Image(_) => ParityCategory::Texture,
        PropValue::Shader(_) => ParityCategory::Shader,
        PropValue::Mesh(_) => ParityCategory::Mesh,
        PropValue::Animation(_) => ParityCategory::Animation,
        _ => component_category(component),
    }
}

enum ExpectedField {
    Exact(String),
    Number(f32),
    BoundAsset(String),
    Ignore,
}

fn expected_prop(prop: &Prop) -> ExpectedField {
    if prop.optional
        && matches!(
            &prop.value,
            PropValue::Text(path)
                | PropValue::Font(path)
                | PropValue::Image(path)
                | PropValue::Sound(path)
                | PropValue::Mesh(path)
                | PropValue::Material(path)
                | PropValue::PhysicsMaterial(path)
                | PropValue::Shader(path)
                | PropValue::Animation(path)
                if path.is_empty()
        )
    {
        return ExpectedField::Ignore;
    }
    match &prop.value {
        PropValue::Number(value) => ExpectedField::Number(*value),
        PropValue::Int(value) => ExpectedField::Exact(value.to_string()),
        PropValue::Bool(value) => ExpectedField::Exact(value.to_string()),
        PropValue::Text(value) | PropValue::Font(value) | PropValue::Mesh(value) => {
            ExpectedField::Exact(value.clone())
        }
        PropValue::Enum { value, .. } => ExpectedField::Exact(value.clone()),
        PropValue::Color([r, g, b, a]) => {
            ExpectedField::Exact(format!("Color4({r}, {g}, {b}, {a})"))
        }
        PropValue::Image(path)
        | PropValue::Sound(path)
        | PropValue::Material(path)
        | PropValue::PhysicsMaterial(path)
        | PropValue::Shader(path)
        | PropValue::Animation(path) => ExpectedField::BoundAsset(path.clone()),
        PropValue::StringList(_)
        | PropValue::ColorSequence(_)
        | PropValue::NumberSequence(_) => ExpectedField::Ignore,
    }
}

fn expected_var(value: &VarValue) -> ExpectedField {
    match value {
        VarValue::Number(value) => ExpectedField::Number(*value),
        VarValue::Bool(value) => ExpectedField::Exact(value.to_string()),
        VarValue::Text(value) => ExpectedField::Exact(value.clone()),
        VarValue::Color([r, g, b, a]) => {
            ExpectedField::Exact(format!("Color4({r}, {g}, {b}, {a})"))
        }
        VarValue::Image(path)
        | VarValue::Audio(path)
        | VarValue::Shader(path)
        | VarValue::Animation(path) => ExpectedField::BoundAsset(path.clone()),
        VarValue::Entity(_)
        | VarValue::Component(_)
        | VarValue::List(_)
        | VarValue::Dictionary(_) => ExpectedField::Ignore,
    }
}

fn compare_expected_field(
    report: &mut ParityReport,
    category: ParityCategory,
    entity_id: u64,
    component_index: usize,
    component: &str,
    property: &str,
    expected: ExpectedField,
    actual: Option<&str>,
) {
    let matches = match &expected {
        ExpectedField::Exact(expected) => actual == Some(expected.as_str()),
        ExpectedField::Number(expected) => actual.is_some_and(|actual| number_equal(*expected, actual)),
        ExpectedField::BoundAsset(path) => {
            path.is_empty() || actual.is_some_and(|value| value != "nil" && !value.is_empty())
        }
        ExpectedField::Ignore => return,
    };
    if matches {
        return;
    }
    let expected = match expected {
        ExpectedField::Exact(value) => value,
        ExpectedField::Number(value) => value.to_string(),
        ExpectedField::BoundAsset(path) => format!("bound asset {path}"),
        ExpectedField::Ignore => unreachable!(),
    };
    mismatch(
        report,
        category,
        Some(entity_id),
        Some(component_index),
        Some(component),
        Some(property),
        expected,
        actual.unwrap_or("<missing>"),
    );
}

/// Compare an authored 3D scene with the real runtime's immutable initial
/// snapshot. The result is structured so every mismatch can link back to its
/// entity, component slot, and property in the editor.
pub(crate) fn validate(scene: &Scene, runtime: &[EntitySnapshot]) -> ParityReport {
    let mut report = ParityReport::default();
    if scene.kind != SceneKind::ThreeD {
        mismatch(
            &mut report,
            ParityCategory::Serialization,
            None,
            None,
            None,
            Some("scene.kind"),
            "3d",
            "2d",
        );
        return report;
    }

    let authored = scene
        .entities
        .iter()
        .filter(|entity| scene.is_active_in_tree(entity.id))
        .map(|entity| (entity.id, entity))
        .collect::<BTreeMap<_, _>>();
    report.authored_entities = authored.len();
    report.runtime_entities = runtime.len();

    let runtime_id_to_source = runtime
        .iter()
        .filter_map(|entity| entity.source_id.map(|source| (entity.id, source)))
        .collect::<HashMap<_, _>>();
    let mut runtime_by_source = BTreeMap::new();
    for entity in runtime {
        let Some(source_id) = entity.source_id else {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                None,
                None,
                None,
                Some("source_id"),
                "authored source id",
                format!("runtime-only #{} ({})", entity.id, entity.name),
            );
            continue;
        };
        if runtime_by_source.insert(source_id, entity).is_some() {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                Some(source_id),
                None,
                None,
                Some("source_id"),
                "unique",
                "duplicate runtime source id",
            );
        }
    }

    for (&entity_id, entity) in &authored {
        let Some(live) = runtime_by_source.get(&entity_id).copied() else {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                Some(entity_id),
                None,
                None,
                None,
                "present in runtime",
                "missing",
            );
            continue;
        };
        if live.name != entity.name {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                Some(entity_id),
                None,
                None,
                Some("name"),
                &entity.name,
                &live.name,
            );
        }
        if !live.enabled {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                Some(entity_id),
                None,
                None,
                Some("enabled"),
                "true",
                "false",
            );
        }

        let live_parent = live
            .parent
            .and_then(|parent| runtime_id_to_source.get(&parent).copied());
        if live_parent != entity.parent {
            mismatch(
                &mut report,
                ParityCategory::Hierarchy,
                Some(entity_id),
                None,
                None,
                Some("parent"),
                entity.parent.map_or_else(|| "root".to_string(), |id| format!("#{id}")),
                live_parent.map_or_else(|| "root".to_string(), |id| format!("#{id}")),
            );
        }

        let live_fields = fields(live);
        compare_number_value(&mut report, entity_id, "x", entity.x, live.x);
        compare_number_value(&mut report, entity_id, "y", entity.y, live.y);
        compare_number(
            &mut report,
            entity_id,
            "position_z",
            entity.position_z,
            live_fields.get("position_z").copied(),
        );
        compare_number_value(
            &mut report,
            entity_id,
            "rotation",
            entity.rotation,
            live.rotation,
        );
        for (property, expected) in [
            ("rotation_x", entity.rotation_x),
            ("rotation_y", entity.rotation_y),
            ("rotation_z", entity.rotation_z),
            ("scale_x", entity.scale_x),
            ("scale_y", entity.scale_y),
            ("scale_z", entity.scale_z),
        ] {
            compare_number(
                &mut report,
                entity_id,
                property,
                expected,
                live_fields.get(property).copied(),
            );
        }
        compare_number_value(&mut report, entity_id, "scale", entity.scale, live.scale);

        let live_components = live
            .components
            .iter()
            .filter_map(|component| component.source_index.map(|index| (index, component)))
            .collect::<BTreeMap<_, _>>();
        for (index, component) in entity.components.iter().enumerate() {
            let label = component.label();
            let Some(live_component) = live_components.get(&index).copied() else {
                mismatch(
                    &mut report,
                    component_category(component),
                    Some(entity_id),
                    Some(index),
                    Some(label),
                    None,
                    "present in runtime",
                    "missing",
                );
                continue;
            };
            let expected_key = authored_component_key(component);
            if live_component.source_key.as_deref() != Some(expected_key.as_str()) {
                mismatch(
                    &mut report,
                    component_category(component),
                    Some(entity_id),
                    Some(index),
                    Some(label),
                    Some("component identity"),
                    expected_key,
                    live_component.source_key.as_deref().unwrap_or("<missing>"),
                );
            }
            let live_fields = component_fields(live_component);
            match component {
                Component::Core { props, .. } => {
                    for prop in props {
                        compare_expected_field(
                            &mut report,
                            property_category(component, prop),
                            entity_id,
                            index,
                            label,
                            &prop.name,
                            expected_prop(prop),
                            live_fields.get(prop.name.as_str()).copied(),
                        );
                    }
                }
                Component::Script { variables, .. } => {
                    for variable in variables {
                        compare_expected_field(
                            &mut report,
                            ParityCategory::Script,
                            entity_id,
                            index,
                            label,
                            &variable.name,
                            expected_var(&variable.value),
                            live_fields.get(variable.name.as_str()).copied(),
                        );
                    }
                }
            }
        }
        for (&index, live_component) in &live_components {
            if index >= entity.components.len() {
                mismatch(
                    &mut report,
                    ParityCategory::Component,
                    Some(entity_id),
                    Some(index),
                    Some(&live_component.name),
                    None,
                    "no extra component",
                    "runtime-only component",
                );
            }
        }
        if live.components.iter().any(|component| component.source_index.is_none()) {
            mismatch(
                &mut report,
                ParityCategory::Component,
                Some(entity_id),
                None,
                None,
                Some("component source index"),
                "all authored components linked",
                "runtime-only or unlinked component",
            );
        }
    }

    for source_id in runtime_by_source.keys() {
        if !authored.contains_key(source_id) {
            mismatch(
                &mut report,
                ParityCategory::Serialization,
                Some(*source_id),
                None,
                None,
                Some("source_id"),
                "active authored entity",
                "unexpected runtime source id",
            );
        }
    }
    report.mismatches.sort_by(|a, b| {
        (
            a.category,
            a.entity_id,
            a.component_index,
            a.property.as_deref(),
        )
            .cmp(&(
                b.category,
                b.entity_id,
                b.component_index,
                b.property.as_deref(),
            ))
    });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Entity, SceneKind};

    fn runtime_entity(entity: &Entity, runtime_id: usize) -> EntitySnapshot {
        EntitySnapshot {
            id: runtime_id,
            source_id: Some(entity.id),
            name: entity.name.clone(),
            parent: entity.parent.map(|id| id as usize + 100),
            x: entity.x,
            y: entity.y,
            rotation: entity.rotation,
            scale: entity.scale,
            enabled: true,
            fields: vec![
                ("position_z".into(), entity.position_z.to_string()),
                ("rotation_x".into(), entity.rotation_x.to_string()),
                ("rotation_y".into(), entity.rotation_y.to_string()),
                ("rotation_z".into(), entity.rotation_z.to_string()),
                ("scale_x".into(), entity.scale_x.to_string()),
                ("scale_y".into(), entity.scale_y.to_string()),
                ("scale_z".into(), entity.scale_z.to_string()),
            ],
            components: entity
                .components
                .iter()
                .enumerate()
                .map(|(index, component)| ComponentSnapshot {
                    name: component.label().to_string(),
                    source_index: Some(index),
                    source_key: Some(authored_component_key(component)),
                    fields: match component {
                        Component::Core { props, .. } => props
                            .iter()
                            .filter_map(|prop| match expected_prop(prop) {
                                ExpectedField::Exact(value) => Some((prop.name.clone(), value)),
                                ExpectedField::Number(value) => {
                                    Some((prop.name.clone(), value.to_string()))
                                }
                                ExpectedField::BoundAsset(path) if !path.is_empty() => {
                                    Some((prop.name.clone(), format!("Handle({path})")))
                                }
                                _ => None,
                            })
                            .collect(),
                        Component::Script { .. } => Vec::new(),
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn exact_initial_runtime_snapshot_passes() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        scene.entities.truncate(1);
        scene.entities[0].components.clear();
        scene.entities[0].position_z = 3.5;
        let runtime = vec![runtime_entity(&scene.entities[0], 101)];
        let report = validate(&scene, &runtime);
        assert!(report.is_match(), "{:#?}", report.mismatches);
    }

    #[test]
    fn classifies_transform_hierarchy_component_and_asset_mismatches() {
        let mut scene = Scene::new_for_kind(SceneKind::ThreeD);
        scene.entities.clear();
        let mut parent = Entity::new(7, "Parent", 1.0, 2.0);
        parent.components.push(Component::core("MeshRenderer3D"));
        let mut child = Entity::new(9, "Child", 4.0, 5.0);
        child.parent = Some(7);
        let mut collider = Component::core("Collider3D");
        if let Component::Core { props, .. } = &mut collider {
            props
                .iter_mut()
                .find(|prop| prop.name == "physics_material")
                .expect("physics material property")
                .value = PropValue::PhysicsMaterial(
                "assets/materials/ice.neophysicsmaterial".into(),
            );
        }
        child.components.push(collider);
        let mut rigidbody = Component::core("Rigidbody3D");
        if let Component::Core { props, .. } = &mut rigidbody {
            props
                .iter_mut()
                .find(|prop| prop.name == "continuous_collision")
                .expect("CCD property")
                .value = PropValue::Bool(true);
        }
        child.components.push(rigidbody);
        scene.entities.push(parent);
        scene.entities.push(child);

        let mut live_parent = runtime_entity(&scene.entities[0], 107);
        live_parent.x = 99.0;
        live_parent.components[0].source_key = Some("core:Camera3D".into());
        live_parent.components[0]
            .fields
            .retain(|(name, _)| name != "mesh_path");
        let mut live_child = runtime_entity(&scene.entities[1], 109);
        live_child.parent = None;
        live_child.components[0]
            .fields
            .retain(|(name, _)| name != "physics_material");
        live_child.components[1]
            .fields
            .retain(|(name, _)| name != "continuous_collision");
        let report = validate(&scene, &[live_parent, live_child]);

        assert!(report.mismatches.iter().any(|mismatch| {
            mismatch.category == ParityCategory::Transform
                && mismatch.entity_id == Some(7)
                && mismatch.property.as_deref() == Some("x")
        }));
        assert!(report.mismatches.iter().any(|mismatch| {
            mismatch.category == ParityCategory::Hierarchy
                && mismatch.entity_id == Some(9)
                && mismatch.property.as_deref() == Some("parent")
        }));
        assert!(report.mismatches.iter().any(|mismatch| {
            mismatch.category == ParityCategory::Mesh
                && mismatch.component_index == Some(0)
                && mismatch.property.as_deref() == Some("component identity")
        }));
        assert!(report.mismatches.iter().any(|mismatch| {
            mismatch.category == ParityCategory::Physics
                && mismatch.entity_id == Some(9)
                && mismatch.property.as_deref() == Some("physics_material")
        }));
        assert!(report.mismatches.iter().any(|mismatch| {
            mismatch.category == ParityCategory::Physics
                && mismatch.entity_id == Some(9)
                && mismatch.component_index == Some(1)
                && mismatch.property.as_deref() == Some("continuous_collision")
        }));
    }
}
