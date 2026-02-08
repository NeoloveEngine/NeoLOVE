use macroquad::audio::{play_sound, set_sound_volume, stop_sound, PlaySoundParams};
use mlua::{AnyUserData, Lua};

pub(crate) fn add_audio_module(lua: &Lua) -> mlua::Result<()> {
    let audio = lua.create_table()?;

    audio.set(
        "play",
        lua.create_function(
            move |_lua, (sound_ud, looped, volume): (AnyUserData, Option<bool>, Option<f32>)| {
                let sound = sound_ud.borrow::<crate::assets::SoundHandle>()?;
                sound.ensure_uploaded()?;

                let Some(mq_sound) = sound.sound() else {
                    return Err(mlua::Error::external("sound is not ready"));
                };

                play_sound(
                    &mq_sound,
                    PlaySoundParams {
                        looped: looped.unwrap_or(false),
                        volume: volume.unwrap_or(1.0).clamp(0.0, 1.0),
                    },
                );
                Ok(())
            },
        )?,
    )?;

    audio.set(
        "playOnce",
        lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, Option<f32>)| {
            let sound = sound_ud.borrow::<crate::assets::SoundHandle>()?;
            sound.ensure_uploaded()?;

            let Some(mq_sound) = sound.sound() else {
                return Err(mlua::Error::external("sound is not ready"));
            };

            play_sound(
                &mq_sound,
                PlaySoundParams {
                    looped: false,
                    volume: volume.unwrap_or(1.0).clamp(0.0, 1.0),
                },
            );
            Ok(())
        })?,
    )?;

    audio.set(
        "stop",
        lua.create_function(move |_lua, sound_ud: AnyUserData| {
            let sound = sound_ud.borrow::<crate::assets::SoundHandle>()?;
            sound.ensure_uploaded()?;

            let Some(mq_sound) = sound.sound() else {
                return Err(mlua::Error::external("sound is not ready"));
            };

            stop_sound(&mq_sound);
            Ok(())
        })?,
    )?;

    audio.set(
        "setVolume",
        lua.create_function(move |_lua, (sound_ud, volume): (AnyUserData, f32)| {
            let sound = sound_ud.borrow::<crate::assets::SoundHandle>()?;
            sound.ensure_uploaded()?;

            let Some(mq_sound) = sound.sound() else {
                return Err(mlua::Error::external("sound is not ready"));
            };

            set_sound_volume(&mq_sound, volume.clamp(0.0, 1.0));
            Ok(())
        })?,
    )?;

    lua.globals().set("audio", audio)?;
    Ok(())
}
