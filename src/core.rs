use macroquad::color::Color;
use macroquad::prelude::vec2;
use macroquad::shapes::draw_rectangle;
use macroquad::text::draw_text;
use macroquad::texture::{draw_texture_ex, DrawTextureParams};
use mlua::{AnyUserData, Lua, Table};

fn color4(lua: &Lua, r: u8, g: u8, b: u8, a: u8) -> mlua::Result<Table> {
    let color = lua.create_table()?;
    color.set("r", r)?;
    color.set("g", g)?;
    color.set("b", b)?;
    color.set("a", a)?;
    Ok(color)
}

fn color4_to_color(color4: Table) -> mlua::Result<Color> {
    let r: f32 = color4.get("r")?;
    let g: f32 = color4.get("g")?;
    let b: f32 = color4.get("b")?;
    let a: f32 = color4.get("a")?;
    Ok(Color::from_rgba(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
        a.clamp(0.0, 255.0) as u8,
    ))
}

fn create_basic_drawable(lua: &Lua) -> mlua::Result<Table> {
    let drawable = lua.create_table()?;
    drawable
        .set(
            "awake",
            lua.create_function(move |ctx, (_entity, component): (Table, Table)| {
                component.set("color", color4(ctx, 255, 255, 255, 255)?)?;
                Ok(())
            })?,
        )?;
    drawable.set("NEOLOVE_RENDERING", true)?;
    Ok(drawable)
}

pub fn add_core_components(lua: &Lua) -> mlua::Result<()> {
    let core_components = lua.create_table()?;

    // Color4
    // not a component!? helper function to generate color4 values
    {
        lua.globals().set(
            "Color4",
            lua.create_function(
                move |ctx, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
                    let alpha: f32 = a.unwrap_or(255.0);
                    color4(
                        ctx,
                        r.clamp(0.0, 255.0) as u8,
                        g.clamp(0.0, 255.0) as u8,
                        b.clamp(0.0, 255.0) as u8,
                        alpha.clamp(0.0, 255.0) as u8,
                    )
                },
            )?,
        )?;
    }

    // Rect2d
    // basic renderer
    {
        let rect2d = create_basic_drawable(lua)?;
        rect2d.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                let (x, y) = crate::window::get_global_position(&entity)?;
                let w: f32 = entity.get("size_x")?;
                let h: f32 = entity.get("size_y")?;
                let color = color4_to_color(component.get("color")?)?;
                draw_rectangle(x, y, w, h, color);
                Ok(())
            })?,
        )?;

        core_components.set("Rect2D", rect2d)?;
    }

    // RudimentaryTextLabel
    // a basic text label for games to use
    // should be replaced by a custom text renderer in proper applications
    {
        let textlabel = create_basic_drawable(lua)?;
        textlabel.set("text", "Text Label")?;
        textlabel.set("scale", 32)?;

        textlabel.set("dx", 0)?;
        textlabel.set("dy", 0)?;

        textlabel.set(
            "update",
            lua.create_function(move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                let (x, y) = crate::window::get_global_position(&entity)?;
                let text: String = component.get("text")?;
                let scale: f32 = component.get("scale")?;
                let color: Color = color4_to_color(component.get("color")?)?;

                let dimensions = draw_text(text.as_str(), x, y, scale, color);
                component.set("dx", dimensions.width)?;
                component.set("dy", dimensions.height)?;

                Ok(())
            })?,
        )?;

        core_components.set("RudimentaryTextLabel", textlabel)?;
    }

    // Image2D
    // draw an image (texture) tinted by component.color, scaled to entity size
    {
        let image2d = create_basic_drawable(lua)?;

        image2d
            .set(
                "update",
                lua.create_function(
                    move |_ctx, (entity, component, _dt): (Table, Table, f32)| {
                        let (x, y) = crate::window::get_global_position(&entity)?;
                        let w: f32 = entity.get("size_x")?;
                        let h: f32 = entity.get("size_y")?;

                        let tint: Color = color4_to_color(component.get("color")?)?;
                        let image: Option<AnyUserData> = component.get("image")?;
                        let Some(image) = image else {
                            return Ok(());
                        };

                        let image = image.borrow::<crate::assets::ImageHandle>()?;
                        image.ensure_uploaded();
                        let texture = image.texture();

                        draw_texture_ex(
                            &texture,
                            x,
                            y,
                            tint,
                            DrawTextureParams {
                                dest_size: Some(vec2(w, h)),
                                ..Default::default()
                            },
                        );

                        Ok(())
                    },
                )?,
            )?;

        core_components.set("Image2D", image2d)?;
    }

    lua.globals().set("core", core_components)?;
    Ok(())
}
