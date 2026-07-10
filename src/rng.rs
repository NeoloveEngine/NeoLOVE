#![allow(dead_code)]

//! Seedable random-number generators as first-class Luau objects.
//!
//! `math.random` shares one hidden global stream, which makes reproducible
//! runs (procedural generation, replays, tests) awkward. `Rng` instead hands
//! out independent generators you can seed and advance in isolation:
//!
//! ```luau
//! local rng = Rng.new(1234)      -- seeded, reproducible
//! local noise = Rng.new()        -- entropy-seeded
//! print(rng:integer(1, 6))       -- a dice roll
//! print(rng:number())            -- a float in [0, 1)
//! rng:shuffle(deck)              -- in-place Fisher-Yates
//! ```
//!
//! Each instance is a xoshiro256** stream seeded through SplitMix64, exposed as
//! Lua userdata with camelCase and PascalCase method spellings.

use mlua::{Lua, MultiValue, Table, UserData, UserDataMethods, Value, Variadic};
use std::sync::atomic::{AtomicU64, Ordering};

/// A xoshiro256** generator. Small, fast, and good enough for gameplay.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rng {
    state: [u64; 4],
}

#[inline]
fn rotl(x: u64, k: u32) -> u64 {
    (x << k) | (x >> (64 - k))
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A best-effort entropy seed for unseeded generators. Mixes the wall clock
/// with a monotonically increasing counter so two `Rng.new()` calls in the same
/// nanosecond still diverge.
fn entropy_seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678_9ABC_DEF0);
    let mut mixed = time ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    splitmix64(&mut mixed)
}

/// Hash an arbitrary string into a 64-bit seed (FNV-1a) so `Rng.fromString`
/// gives stable, reproducible seeds from names.
fn hash_string(s: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

impl Rng {
    pub(crate) fn from_seed(seed: u64) -> Self {
        let mut sm = seed;
        let state = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        // SplitMix64 never yields an all-zero run for distinct outputs, but guard
        // against a degenerate state anyway.
        if state == [0, 0, 0, 0] {
            return Self::from_seed(seed ^ 0xDEAD_BEEF);
        }
        Self { state }
    }

    pub(crate) fn from_entropy() -> Self {
        Self::from_seed(entropy_seed())
    }

    fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = rotl(s[1].wrapping_mul(5), 7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = rotl(s[3], 45);
        result
    }

    /// A float in `[0, 1)` with 53 bits of resolution.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Inclusive integer in `[min, max]` (order-insensitive).
    fn next_range_i64(&mut self, min: i64, max: i64) -> i64 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        let span = (hi as i128 - lo as i128 + 1) as u128;
        if span <= 1 {
            return lo;
        }
        let r = (self.next_u64() as u128) % span;
        (lo as i128 + r as i128) as i64
    }
}

impl UserData for Rng {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Float in [0, 1).
        methods.add_method_mut("next", |_, this, ()| Ok(this.next_f64()));

        // number()          -> [0, 1)
        // number(max)       -> [0, max)
        // number(min, max)  -> [min, max)
        let number = |_: &Lua, this: &mut Rng, args: Variadic<f64>| -> mlua::Result<f64> {
            let value = match args.len() {
                0 => this.next_f64(),
                1 => this.next_f64() * args[0],
                _ => {
                    let (lo, hi) = (args[0], args[1]);
                    lo + this.next_f64() * (hi - lo)
                }
            };
            Ok(value)
        };
        methods.add_method_mut("number", number);
        methods.add_method_mut("float", number);
        methods.add_method_mut("range", number);

        // integer(min, max) inclusive. integer(max) means [1, max].
        let integer = |_: &Lua, this: &mut Rng, args: Variadic<i64>| -> mlua::Result<i64> {
            let value = match args.len() {
                0 => this.next_u64() as i64,
                1 => this.next_range_i64(1, args[0]),
                _ => this.next_range_i64(args[0], args[1]),
            };
            Ok(value)
        };
        methods.add_method_mut("integer", integer);
        methods.add_method_mut("int", integer);

        // boolean(p?) — true with probability p (default 0.5).
        methods.add_method_mut("boolean", |_, this, p: Option<f64>| {
            let threshold = p.unwrap_or(0.5);
            Ok(this.next_f64() < threshold)
        });
        methods.add_method_mut("bool", |_, this, p: Option<f64>| {
            let threshold = p.unwrap_or(0.5);
            Ok(this.next_f64() < threshold)
        });

        // sign() -> -1 or 1.
        methods.add_method_mut("sign", |_, this, ()| {
            Ok(if this.next_u64() & 1 == 0 { -1 } else { 1 })
        });

        // angle() -> radians in [0, 2pi).
        methods.add_method_mut("angle", |_, this, ()| {
            Ok(this.next_f64() * std::f64::consts::TAU)
        });

        // unit() -> a random unit vector (x, y).
        methods.add_method_mut("unit", |_, this, ()| {
            let theta = this.next_f64() * std::f64::consts::TAU;
            Ok((theta.cos(), theta.sin()))
        });

        // pick(list) -> a uniformly random element of an array-like table.
        methods.add_method_mut("pick", |_, this, list: Table| {
            let len = list.raw_len();
            if len == 0 {
                return Ok(Value::Nil);
            }
            let index = this.next_range_i64(1, len as i64);
            list.get::<Value>(index)
        });

        // shuffle(list) -> the same table, shuffled in place (Fisher-Yates).
        methods.add_method_mut("shuffle", |_, this, list: Table| {
            let len = list.raw_len() as i64;
            let mut i = len;
            while i > 1 {
                let j = this.next_range_i64(1, i);
                if j != i {
                    let a: Value = list.get(i)?;
                    let b: Value = list.get(j)?;
                    list.set(i, b)?;
                    list.set(j, a)?;
                }
                i -= 1;
            }
            Ok(list)
        });

        // seed(n) — reseed this generator in place.
        methods.add_method_mut("seed", |_, this, seed: i64| {
            *this = Rng::from_seed(seed as u64);
            Ok(())
        });

        // clone() -> an independent copy at the same position in the stream.
        methods.add_method("clone", |lua, this, ()| lua.create_userdata(*this));
        methods.add_method("Clone", |lua, this, ()| lua.create_userdata(*this));
    }
}

/// Build the `Rng` module table (`Rng.new`, `Rng.fromString`), which is also
/// directly callable: `Rng(seed)` is shorthand for `Rng.new(seed)`.
pub(crate) fn create_module(lua: &Lua) -> mlua::Result<Table> {
    let module = lua.create_table()?;

    module.set(
        "new",
        lua.create_function(|lua, seed: Option<i64>| {
            let rng = match seed {
                Some(seed) => Rng::from_seed(seed as u64),
                None => Rng::from_entropy(),
            };
            lua.create_userdata(rng)
        })?,
    )?;

    module.set(
        "fromString",
        lua.create_function(|lua, text: String| {
            lua.create_userdata(Rng::from_seed(hash_string(&text)))
        })?,
    )?;

    // Make the module callable: `Rng(seed?)` == `Rng.new(seed?)`.
    let metatable = lua.create_table()?;
    metatable.set(
        "__call",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args;
            // Drop the table itself (the `self` passed for a `Rng(...)` call).
            let _ = args.pop_front();
            let seed = match args.pop_front() {
                Some(Value::Integer(n)) => Some(n),
                Some(Value::Number(n)) => Some(n as i64),
                _ => None,
            };
            let rng = match seed {
                Some(seed) => Rng::from_seed(seed as u64),
                None => Rng::from_entropy(),
            };
            lua.create_userdata(rng)
        })?,
    )?;
    module.set_metatable(Some(metatable))?;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_reproduces_sequence() {
        let mut a = Rng::from_seed(42);
        let mut b = Rng::from_seed(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(2);
        let diffs = (0..64).filter(|_| a.next_u64() != b.next_u64()).count();
        assert!(diffs > 60, "streams should differ (differed {diffs}/64)");
    }

    #[test]
    fn range_is_inclusive_and_bounded() {
        let mut rng = Rng::from_seed(7);
        for _ in 0..10_000 {
            let v = rng.next_range_i64(3, 8);
            assert!((3..=8).contains(&v), "value {v} out of range");
        }
        // Reversed bounds behave the same.
        for _ in 0..1000 {
            let v = rng.next_range_i64(8, 3);
            assert!((3..=8).contains(&v));
        }
    }

    #[test]
    fn floats_stay_in_unit_interval() {
        let mut rng = Rng::from_seed(99);
        for _ in 0..10_000 {
            let f = rng.next_f64();
            assert!((0.0..1.0).contains(&f), "float {f} out of [0,1)");
        }
    }

    #[test]
    fn from_string_is_stable() {
        let mut a = Rng::from_seed(hash_string("level-1"));
        let mut b = Rng::from_seed(hash_string("level-1"));
        assert_eq!(a.next_u64(), b.next_u64());
        let mut c = Rng::from_seed(hash_string("level-2"));
        assert_ne!(a.next_u64(), c.next_u64());
    }
}
