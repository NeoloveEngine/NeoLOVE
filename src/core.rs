use macroquad::color::Color;
use macroquad::color_u8;
use macroquad::shapes::draw_rectangle;
use macroquad::text::draw_text;
use mlua::{Lua, Table};

fn color4(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> Table {
    let color = lua.create_table().unwrap();
    color.set("r", r).unwrap();
    color.set("g", g).unwrap();
    color.set("b", b).unwrap();
    color.set("a", a).unwrap();
    color
}

fn color4_to_color(color4: Table) -> Color {
    let r: f32 = color4.get("r").unwrap();
    let g: f32 = color4.get("g").unwrap();
    let b: f32 = color4.get("b").unwrap();
    let a: f32 = color4.get("a").unwrap();

    color_u8!(r, g, b, a)
}

fn create_basic_drawable(lua: &Lua) -> Table {
    let drawable = lua.create_table().unwrap();
    drawable
        .set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component
                    .set("color", color4(ctx, 255, 255, 255, 255))
                    .unwrap();
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
    drawable.set("NEOLOVE_RENDERING", true).unwrap();
    drawable
}

pub fn add_core_components(lua: &Lua) {
    let core_components = lua.create_table().unwrap();

    // Color4
    // not a component!? helper function to generate color4 values
    {
        lua.globals().set("Color4",
            lua.create_function(
                move |ctx, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
                    let mut alpha: f32 = 255f32;
                    if a.is_some() {
                        alpha = a.unwrap();
                    }

                    Ok(color4(
                        ctx,
                        r.clamp(0f32,255f32) as u8, g.clamp(0f32, 255f32) as u8, b.clamp(0f32, 255f32) as u8,
                        alpha.clamp(0f32,255f32) as u8,
                    ))
                }
            ).unwrap()
        ).unwrap();
    }

    // Rect2d
    // basic renderer
    {
        let rect2d = create_basic_drawable(lua);
        rect2d.set("update", lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
            let (x, y) = crate::window::get_global_position(&entity).unwrap();
            let (w, h): (f32, f32) = (entity.get("size_x").unwrap(), entity.get("size_y").unwrap());
            draw_rectangle(x, y, w, h, color4_to_color(component.get("color").unwrap()));

            Ok(())
        }).unwrap()).unwrap();

        core_components.set("Rect2D", rect2d).unwrap();
    }

    // RudimentaryTextLabel
    // a basic text label for games to use
    // should be replaced by a custom text renderer in proper applications
    {
        let textlabel = create_basic_drawable(lua);
        textlabel.set("text", "Text Label").unwrap();
        textlabel.set("scale", 32).unwrap();

        textlabel.set("dx", 0).unwrap();
        textlabel.set("dy", 0).unwrap();

        textlabel.set("update", lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
            let (x, y) = crate::window::get_global_position(&entity).unwrap();
            let text: String = component.get("text").unwrap();
            let scale: f32 = component.get("scale").unwrap();
            let color: Color = color4_to_color(component.get("color").unwrap());

            let dimensions = draw_text(text.as_str(), x, y, scale, color);
            component.set("dx", dimensions.width).unwrap();
            component.set("dy", dimensions.height).unwrap();

            Ok(())
        }).unwrap()).unwrap();

        core_components.set("RudimentaryTextLabel", textlabel).unwrap();
    }

    lua.globals().set("core", core_components).unwrap();
}
