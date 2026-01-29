use mlua::RegistryKey;

pub type EntityId = usize;

pub struct Entity {
    pub components: Vec<Component>,
    pub children: Vec<EntityId>,
    pub parent: Option<EntityId>,
    pub id: EntityId,
    pub luau_key: RegistryKey,
}

pub struct Component {
    pub name: String,
    pub this: mlua::Table
}