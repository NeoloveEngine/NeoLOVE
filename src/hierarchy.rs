pub type EntityId = usize;

pub struct Entity {
    components: Vec<Component>,
    children: Vec<EntityId>,
    parent: Option<EntityId>
}

pub struct Component {
    name: String,
    this: mlua::Table
}