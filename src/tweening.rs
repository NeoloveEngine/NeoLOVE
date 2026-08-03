use mlua::{Function, Lua, RegistryKey, Table, Value};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EaseStyle {
    Linear,
    Sine,
    Quad,
    Cubic,
    Quart,
    Quint,
    Expo,
    Circ,
    Back,
    Bounce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EaseDirection {
    In,
    Out,
    InOut,
}

struct Tween {
    id: u64,
    target: RegistryKey,
    key: Value,
    from: f64,
    to: f64,
    duration: f64,
    elapsed: f64,
    style: EaseStyle,
    direction: EaseDirection,
    cancelled: bool,
    completed: bool,
    on_complete: Option<RegistryKey>,
}

struct TweenState {
    next_id: u64,
    tweens: Vec<Tween>,
}

fn value_to_number(value: Value, name: &str) -> mlua::Result<f64> {
    match value {
        Value::Integer(value) => Ok(value as f64),
        Value::Number(value) if value.is_finite() => Ok(value),
        Value::Number(_) => Err(mlua::Error::external(format!("{name} must be finite"))),
        _ => Err(mlua::Error::external(format!("{name} must be a number"))),
    }
}

fn parse_style(value: Option<String>) -> mlua::Result<EaseStyle> {
    let Some(value) = value else {
        return Ok(EaseStyle::Linear);
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "linear" => Ok(EaseStyle::Linear),
        "sine" | "sin" => Ok(EaseStyle::Sine),
        "quad" | "quadratic" => Ok(EaseStyle::Quad),
        "cubic" => Ok(EaseStyle::Cubic),
        "quart" | "quartic" => Ok(EaseStyle::Quart),
        "quint" | "quintic" => Ok(EaseStyle::Quint),
        "expo" | "exponential" => Ok(EaseStyle::Expo),
        "circ" | "circular" => Ok(EaseStyle::Circ),
        "back" => Ok(EaseStyle::Back),
        "bounce" => Ok(EaseStyle::Bounce),
        other => Err(mlua::Error::external(format!(
            "unknown easing style '{other}'"
        ))),
    }
}

fn parse_direction(value: Option<String>) -> mlua::Result<EaseDirection> {
    let Some(value) = value else {
        return Ok(EaseDirection::Out);
    };

    match value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "in" => Ok(EaseDirection::In),
        "out" => Ok(EaseDirection::Out),
        "inout" => Ok(EaseDirection::InOut),
        other => Err(mlua::Error::external(format!(
            "unknown easing direction '{other}'"
        ))),
    }
}

fn ease_out_bounce(t: f64) -> f64 {
    const N1: f64 = 7.5625;
    const D1: f64 = 2.75;

    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let t = t - 1.5 / D1;
        N1 * t * t + 0.75
    } else if t < 2.5 / D1 {
        let t = t - 2.25 / D1;
        N1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / D1;
        N1 * t * t + 0.984375
    }
}

fn ease_in(style: EaseStyle, t: f64) -> f64 {
    match style {
        EaseStyle::Linear => t,
        EaseStyle::Sine => 1.0 - (t * std::f64::consts::FRAC_PI_2).cos(),
        EaseStyle::Quad => t * t,
        EaseStyle::Cubic => t * t * t,
        EaseStyle::Quart => t * t * t * t,
        EaseStyle::Quint => t * t * t * t * t,
        EaseStyle::Expo => {
            if t <= 0.0 {
                0.0
            } else {
                2.0_f64.powf(10.0 * t - 10.0)
            }
        }
        EaseStyle::Circ => 1.0 - (1.0 - t * t).sqrt(),
        EaseStyle::Back => {
            const C1: f64 = 1.70158;
            const C3: f64 = C1 + 1.0;
            C3 * t * t * t - C1 * t * t
        }
        EaseStyle::Bounce => 1.0 - ease_out_bounce(1.0 - t),
    }
}

fn ease(style: EaseStyle, direction: EaseDirection, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match direction {
        EaseDirection::In => ease_in(style, t),
        EaseDirection::Out => 1.0 - ease_in(style, 1.0 - t),
        EaseDirection::InOut => {
            if t < 0.5 {
                ease_in(style, t * 2.0) * 0.5
            } else {
                1.0 - ease_in(style, (1.0 - t) * 2.0) * 0.5
            }
        }
    }
}

fn find_active_index(state: &TweenState, id: u64) -> Option<usize> {
    state
        .tweens
        .iter()
        .position(|tween| tween.id == id && !tween.cancelled && !tween.completed)
}

fn create_handle(lua: &Lua, state: Rc<RefCell<TweenState>>, id: u64) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("id", id)?;

    let state_for_cancel = state.clone();
    let cancel = lua.create_function(move |_lua, this: Table| {
        let id = this.get::<u64>("id")?;
        let mut state = state_for_cancel.borrow_mut();
        let Some(index) = find_active_index(&state, id) else {
            return Ok(false);
        };
        state.tweens[index].cancelled = true;
        Ok(true)
    })?;
    handle.set("cancel", cancel.clone())?;
    handle.set("Cancel", cancel)?;

    let state_for_done = state.clone();
    let is_done = lua.create_function(move |_lua, this: Table| {
        let id = this.get::<u64>("id")?;
        let state = state_for_done.borrow();
        Ok(find_active_index(&state, id).is_none())
    })?;
    handle.set("isDone", is_done.clone())?;
    handle.set("IsDone", is_done)?;

    Ok(handle)
}

pub(crate) fn add_tweening_module(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;
    let state = Rc::new(RefCell::new(TweenState {
        next_id: 1,
        tweens: Vec::new(),
    }));

    let state_for_to = state.clone();
    let to = lua.create_function(
        move |lua,
              (target, key, target_value, duration, style, direction, on_complete): (
            Table,
            Value,
            Value,
            f64,
            Option<String>,
            Option<String>,
            Option<Function>,
        )| {
            if !duration.is_finite() || duration < 0.0 {
                return Err(mlua::Error::external("duration must be >= 0"));
            }

            let from = value_to_number(target.raw_get::<Value>(key.clone())?, "current value")?;
            let to_value = value_to_number(target_value, "target value")?;
            let style = parse_style(style)?;
            let direction = parse_direction(direction)?;

            let mut state = state_for_to.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            state.tweens.push(Tween {
                id,
                target: lua.create_registry_value(target)?,
                key,
                from,
                to: to_value,
                duration,
                elapsed: 0.0,
                style,
                direction,
                cancelled: false,
                completed: false,
                on_complete: on_complete
                    .map(|callback| lua.create_registry_value(callback))
                    .transpose()?,
            });
            drop(state);

            Ok(create_handle(lua, state_for_to.clone(), id)?)
        },
    )?;
    module.set("to", to.clone())?;
    module.set("new", to.clone())?;
    module.set("create", to)?;

    let state_for_cancel_all = state.clone();
    let cancel_all = lua.create_function(move |_lua, ()| {
        let mut count = 0usize;
        let mut state = state_for_cancel_all.borrow_mut();
        for tween in &mut state.tweens {
            if !tween.cancelled && !tween.completed {
                tween.cancelled = true;
                count += 1;
            }
        }
        Ok(count)
    })?;
    module.set("cancelAll", cancel_all.clone())?;
    module.set("cancel_all", cancel_all)?;

    let state_for_count = state.clone();
    let count = lua.create_function(move |_lua, ()| {
        let state = state_for_count.borrow();
        Ok(state
            .tweens
            .iter()
            .filter(|tween| !tween.cancelled && !tween.completed)
            .count())
    })?;
    module.set("count", count)?;

    let ease_fn = lua.create_function(
        move |_lua, (t, style, direction): (f64, Option<String>, Option<String>)| {
            Ok(ease(parse_style(style)?, parse_direction(direction)?, t))
        },
    )?;
    module.set("ease", ease_fn)?;

    let state_for_update = state.clone();
    let update = lua.create_function(move |lua, dt: f64| {
        if !dt.is_finite() || dt <= 0.0 {
            return Ok(());
        }

        let mut completed_callbacks = Vec::new();
        {
            let mut state = state_for_update.borrow_mut();
            for tween in &mut state.tweens {
                if tween.cancelled || tween.completed {
                    continue;
                }

                tween.elapsed = (tween.elapsed + dt).min(tween.duration);
                let progress = if tween.duration <= 0.0 {
                    1.0
                } else {
                    tween.elapsed / tween.duration
                };
                let alpha = ease(tween.style, tween.direction, progress);
                let value = tween.from + (tween.to - tween.from) * alpha;

                let target: Table = lua.registry_value(&tween.target)?;
                target.raw_set(tween.key.clone(), value)?;

                if progress >= 1.0 {
                    tween.completed = true;
                    target.raw_set(tween.key.clone(), tween.to)?;
                    if let Some(callback) = &tween.on_complete {
                        completed_callbacks.push(lua.registry_value::<Function>(callback)?);
                    }
                }
            }

            let mut active = Vec::with_capacity(state.tweens.len());
            for tween in state.tweens.drain(..) {
                if tween.cancelled || tween.completed {
                    lua.remove_registry_value(tween.target)?;
                    if let Some(callback) = tween.on_complete {
                        lua.remove_registry_value(callback)?;
                    }
                } else {
                    active.push(tween);
                }
            }
            state.tweens = active;
        }

        for callback in completed_callbacks {
            callback.call::<()>(())?;
        }

        Ok(())
    })?;
    module.set("update", update.clone())?;
    module.set("_update", update)?;

    lua.globals().set("tweening", module.clone())?;
    lua.globals().set("tween", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_out_ease_moves_fast_then_slow() {
        let mid = ease(EaseStyle::Quad, EaseDirection::Out, 0.5);
        assert!((mid - 0.75).abs() < 0.0001);
    }

    #[test]
    fn in_out_ease_is_symmetric() {
        let a = ease(EaseStyle::Cubic, EaseDirection::InOut, 0.25);
        let b = 1.0 - ease(EaseStyle::Cubic, EaseDirection::InOut, 0.75);
        assert!((a - b).abs() < 0.0001);
    }
}
