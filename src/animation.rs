//! Timeline/keyframe animation for Luau tables and entities.
//!
//! Clips are data tables, so the same representation can be authored by the
//! editor, loaded from modules, or assembled at runtime.

use mlua::{Lua, RegistryKey, Table, Value};
use serde::Deserialize;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Copy)]
enum Interpolation {
    Linear,
    Step,
    Bezier,
}

struct Keyframe {
    time: f64,
    value: f64,
    out_x: f64,
    out_y: f64,
    in_x: f64,
    in_y: f64,
}

struct Track {
    property: String,
    interpolation: Interpolation,
    keys: Vec<Keyframe>,
}

struct Player {
    id: u64,
    target: RegistryKey,
    tracks: Vec<Track>,
    duration: f64,
    time: f64,
    speed: f64,
    looping: bool,
    playing: bool,
    finished: bool,
}

struct AnimationState {
    next_id: u64,
    players: Vec<Player>,
}

fn number(value: Value, field: &str) -> mlua::Result<f64> {
    match value {
        Value::Integer(value) => Ok(value as f64),
        Value::Number(value) if value.is_finite() => Ok(value),
        _ => Err(mlua::Error::external(format!(
            "{field} must be a finite number"
        ))),
    }
}

fn optional_number(table: &Table, field: &str, default: f64) -> mlua::Result<f64> {
    match table.get::<Option<Value>>(field)? {
        Some(value) => number(value, field),
        None => Ok(default),
    }
}

fn parse_clip(clip: &Table) -> mlua::Result<(Vec<Track>, f64, bool)> {
    let tracks_table: Table = clip.get("tracks")?;
    let mut tracks = Vec::new();
    let mut inferred_duration = 0.0f64;
    for track in tracks_table.sequence_values::<Table>() {
        let track = track?;
        let property: String = track.get("property")?;
        if property.trim().is_empty() {
            return Err(mlua::Error::external(
                "animation track property cannot be empty",
            ));
        }
        let interpolation = match track
            .get::<Option<String>>("interpolation")?
            .unwrap_or_else(|| "linear".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "linear" => Interpolation::Linear,
            "step" | "hold" => Interpolation::Step,
            "bezier" | "cubic" | "ease" => Interpolation::Bezier,
            other => {
                return Err(mlua::Error::external(format!(
                    "unknown animation interpolation '{other}'"
                )));
            }
        };
        let key_table: Table = track.get("keys")?;
        let mut keys = Vec::new();
        for key in key_table.sequence_values::<Table>() {
            let key = key?;
            let time = number(key.get("time")?, "keyframe time")?;
            if time < 0.0 {
                return Err(mlua::Error::external("keyframe time must be >= 0"));
            }
            let value = number(key.get("value")?, "keyframe value")?;
            let out_x = optional_number(&key, "out_x", 0.333)?.clamp(0.0, 1.0);
            let out_y = optional_number(&key, "out_y", 0.0)?;
            let in_x = optional_number(&key, "in_x", 0.667)?.clamp(0.0, 1.0);
            let in_y = optional_number(&key, "in_y", 1.0)?;
            inferred_duration = inferred_duration.max(time);
            keys.push(Keyframe {
                time,
                value,
                out_x,
                out_y,
                in_x,
                in_y,
            });
        }
        keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        if !keys.is_empty() {
            tracks.push(Track {
                property,
                interpolation,
                keys,
            });
        }
    }
    if tracks.is_empty() {
        return Err(mlua::Error::external("animation clip has no keyframes"));
    }
    let duration = clip
        .get::<Option<f64>>("duration")?
        .unwrap_or(inferred_duration)
        .max(inferred_duration);
    let looping = clip
        .get::<Option<bool>>("looping")?
        .or(clip.get::<Option<bool>>("looped")?)
        .unwrap_or(false);
    Ok((tracks, duration, looping))
}

fn cubic_bezier(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let mt = 1.0 - t;
    mt * mt * mt * a + 3.0 * mt * mt * t * b + 3.0 * mt * t * t * c + t * t * t * d
}

fn sample_bezier(from: &Keyframe, to: &Keyframe, alpha: f64) -> f64 {
    // Handles are stored in normalized segment space. Invert x(t) with a short
    // binary search so non-linear time handles work predictably.
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        let x = cubic_bezier(0.0, from.out_x, to.in_x, 1.0, mid);
        if x < alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = (lo + hi) * 0.5;
    let y = cubic_bezier(0.0, from.out_y, to.in_y, 1.0, t);
    from.value + (to.value - from.value) * y
}

fn sample(track: &Track, time: f64) -> f64 {
    let first = &track.keys[0];
    if time <= first.time {
        return first.value;
    }
    let last = track.keys.last().expect("track is non-empty");
    if time >= last.time {
        return last.value;
    }
    for pair in track.keys.windows(2) {
        let from = &pair[0];
        let to = &pair[1];
        if time <= to.time {
            let span = (to.time - from.time).max(f64::EPSILON);
            let alpha = ((time - from.time) / span).clamp(0.0, 1.0);
            return match track.interpolation {
                Interpolation::Step => from.value,
                Interpolation::Bezier => sample_bezier(from, to, alpha),
                Interpolation::Linear => from.value + (to.value - from.value) * alpha,
            };
        }
    }
    last.value
}

#[derive(Deserialize)]
struct JsonClip {
    duration: Option<f64>,
    looping: Option<bool>,
    looped: Option<bool>,
    tracks: Vec<JsonTrack>,
}

#[derive(Deserialize)]
struct JsonTrack {
    property: String,
    interpolation: Option<String>,
    keys: Vec<JsonKeyframe>,
}

#[derive(Deserialize)]
struct JsonKeyframe {
    time: f64,
    value: f64,
    out_x: Option<f64>,
    out_y: Option<f64>,
    in_x: Option<f64>,
    in_y: Option<f64>,
}

fn resolve_path(root: &Path, input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn json_clip_to_lua(lua: &Lua, clip: JsonClip) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    if let Some(duration) = clip.duration {
        table.set("duration", duration)?;
    }
    if let Some(looping) = clip.looping.or(clip.looped) {
        table.set("looping", looping)?;
    }
    let tracks = lua.create_table()?;
    for track in clip.tracks {
        let track_table = lua.create_table()?;
        track_table.set("property", track.property)?;
        track_table.set(
            "interpolation",
            track.interpolation.unwrap_or_else(|| "linear".to_string()),
        )?;
        let keys = lua.create_table()?;
        for key in track.keys {
            let key_table = lua.create_table()?;
            key_table.set("time", key.time)?;
            key_table.set("value", key.value)?;
            if let Some(value) = key.out_x {
                key_table.set("out_x", value)?;
            }
            if let Some(value) = key.out_y {
                key_table.set("out_y", value)?;
            }
            if let Some(value) = key.in_x {
                key_table.set("in_x", value)?;
            }
            if let Some(value) = key.in_y {
                key_table.set("in_y", value)?;
            }
            keys.push(key_table)?;
        }
        track_table.set("keys", keys)?;
        tracks.push(track_table)?;
    }
    table.set("tracks", tracks)?;
    Ok(table)
}

fn with_player<R>(
    state: &Rc<RefCell<AnimationState>>,
    id: u64,
    f: impl FnOnce(&mut Player) -> R,
) -> mlua::Result<R> {
    let mut state = state.borrow_mut();
    let player = state
        .players
        .iter_mut()
        .find(|player| player.id == id)
        .ok_or_else(|| mlua::Error::external("animation player no longer exists"))?;
    Ok(f(player))
}

fn create_handle(lua: &Lua, state: Rc<RefCell<AnimationState>>, id: u64) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("id", id)?;

    for (name, command) in [("play", 0u8), ("pause", 1), ("stop", 2)] {
        let state = state.clone();
        handle.set(
            name,
            lua.create_function(move |_lua, this: Table| {
                let id = this.get("id")?;
                with_player(&state, id, |player| match command {
                    0 => {
                        player.playing = true;
                        player.finished = false;
                    }
                    1 => player.playing = false,
                    _ => {
                        player.playing = false;
                        player.finished = false;
                        player.time = 0.0;
                    }
                })?;
                Ok(())
            })?,
        )?;
    }

    let seek_state = state.clone();
    handle.set(
        "seek",
        lua.create_function(move |_lua, (this, time): (Table, f64)| {
            let id = this.get("id")?;
            with_player(&seek_state, id, |player| {
                player.time = time.clamp(0.0, player.duration);
                player.finished = false;
            })?;
            Ok(())
        })?,
    )?;
    let speed_state = state.clone();
    handle.set(
        "setSpeed",
        lua.create_function(move |_lua, (this, speed): (Table, f64)| {
            if !speed.is_finite() || speed < 0.0 {
                return Err(mlua::Error::external("animation speed must be >= 0"));
            }
            let id = this.get("id")?;
            with_player(&speed_state, id, |player| player.speed = speed)?;
            Ok(())
        })?,
    )?;
    let status_state = state;
    handle.set(
        "isPlaying",
        lua.create_function(move |_lua, this: Table| {
            let id = this.get("id")?;
            with_player(&status_state, id, |player| player.playing)
        })?,
    )?;
    Ok(handle)
}

pub(crate) fn add_animation_module(lua: &Lua, env_root: PathBuf) -> mlua::Result<()> {
    let module = lua.create_table()?;
    let state = Rc::new(RefCell::new(AnimationState {
        next_id: 1,
        players: Vec::new(),
    }));

    let create_state = state.clone();
    let create = lua.create_function(move |lua, (target, clip): (Table, Table)| {
        let (tracks, duration, looping) = parse_clip(&clip)?;
        let mut state = create_state.borrow_mut();
        let id = state.next_id;
        state.next_id += 1;
        state.players.push(Player {
            id,
            target: lua.create_registry_value(target)?,
            tracks,
            duration,
            time: 0.0,
            speed: 1.0,
            looping,
            playing: false,
            finished: false,
        });
        drop(state);
        create_handle(lua, create_state.clone(), id)
    })?;
    module.set("new", create.clone())?;
    module.set("create", create)?;

    let load_root = env_root;
    let load = lua.create_function(move |lua, path: String| {
        let text =
            fs::read_to_string(resolve_path(&load_root, &path)).map_err(mlua::Error::external)?;
        let clip: JsonClip = serde_json::from_str(&text).map_err(mlua::Error::external)?;
        json_clip_to_lua(lua, clip)
    })?;
    module.set("load", load.clone())?;
    module.set("Load", load)?;

    let play_state = state.clone();
    module.set(
        "play",
        lua.create_function(move |lua, (target, clip): (Table, Table)| {
            let (tracks, duration, looping) = parse_clip(&clip)?;
            let mut state = play_state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            state.players.push(Player {
                id,
                target: lua.create_registry_value(target)?,
                tracks,
                duration,
                time: 0.0,
                speed: 1.0,
                looping,
                playing: true,
                finished: false,
            });
            drop(state);
            create_handle(lua, play_state.clone(), id)
        })?,
    )?;

    let update_state = state;
    let update = lua.create_function(move |lua, dt: f64| {
        if !dt.is_finite() || dt < 0.0 {
            return Ok(());
        }
        let mut state = update_state.borrow_mut();
        for player in &mut state.players {
            if player.playing {
                player.time += dt * player.speed;
                if player.duration <= 0.0 || player.time >= player.duration {
                    if player.looping && player.duration > 0.0 {
                        player.time %= player.duration;
                    } else {
                        player.time = player.duration;
                        player.playing = false;
                        player.finished = true;
                    }
                }
            }
            let target: Table = lua.registry_value(&player.target)?;
            for track in &player.tracks {
                target.raw_set(track.property.as_str(), sample(track, player.time))?;
            }
        }
        Ok(())
    })?;
    module.set("update", update.clone())?;
    module.set("_update", update)?;

    lua.globals().set("animation", module.clone())?;
    lua.globals().set("animations", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_linear_tracks() {
        let track = Track {
            property: "x".into(),
            interpolation: Interpolation::Linear,
            keys: vec![
                Keyframe {
                    time: 0.0,
                    value: 2.0,
                    out_x: 0.333,
                    out_y: 0.0,
                    in_x: 0.667,
                    in_y: 1.0,
                },
                Keyframe {
                    time: 2.0,
                    value: 10.0,
                    out_x: 0.333,
                    out_y: 0.0,
                    in_x: 0.667,
                    in_y: 1.0,
                },
            ],
        };
        assert_eq!(sample(&track, 1.0), 6.0);
    }

    #[test]
    fn module_plays_and_controls_clip() -> mlua::Result<()> {
        let lua = Lua::new();
        add_animation_module(&lua, std::env::current_dir().expect("cwd"))?;
        lua.load(
            r#"
            target = { x = 0 }
            player = animation.play(target, {
                duration = 2,
                tracks = {{ property = "x", keys = {
                    { time = 0, value = 10 }, { time = 2, value = 30 }
                }}}
            })
            animation.update(1)
            assert(target.x == 20)
            player:pause()
            animation.update(1)
            assert(target.x == 20)
            player:seek(2)
            animation.update(0)
            assert(target.x == 30)
            "#,
        )
        .exec()
    }
}
