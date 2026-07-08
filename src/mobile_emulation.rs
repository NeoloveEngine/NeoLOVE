use std::env;

pub(crate) const ENV_ENABLED: &str = "NEOLOVE_MOBILE_EMULATOR";
pub(crate) const ENV_WIDTH: &str = "NEOLOVE_MOBILE_WIDTH";
pub(crate) const ENV_HEIGHT: &str = "NEOLOVE_MOBILE_HEIGHT";
pub(crate) const ENV_ORIENTATION: &str = "NEOLOVE_MOBILE_ORIENTATION";
pub(crate) const ENV_WIFI: &str = "NEOLOVE_MOBILE_WIFI";
pub(crate) const ENV_CELLULAR: &str = "NEOLOVE_MOBILE_CELLULAR";
pub(crate) const ENV_LOW_POWER: &str = "NEOLOVE_MOBILE_LOW_POWER";

pub(crate) const DEFAULT_WIDTH: u32 = 390;
pub(crate) const DEFAULT_HEIGHT: u32 = 844;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MobileOrientation {
    Portrait,
    Landscape,
}

impl MobileOrientation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "landscape" | "wide" | "horizontal" => Self::Landscape,
            _ => Self::Portrait,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MobileEmulation {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    pub orientation: MobileOrientation,
    pub wifi: bool,
    pub cellular: bool,
    pub low_power: bool,
}

impl Default for MobileEmulation {
    fn default() -> Self {
        Self {
            enabled: false,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            orientation: MobileOrientation::Portrait,
            wifi: true,
            cellular: false,
            low_power: false,
        }
    }
}

impl MobileEmulation {
    pub(crate) fn from_env() -> Self {
        let mut profile = Self::default();
        profile.enabled = env_bool(ENV_ENABLED, false);
        profile.width = env_u32(ENV_WIDTH, DEFAULT_WIDTH).clamp(120, 4096);
        profile.height = env_u32(ENV_HEIGHT, DEFAULT_HEIGHT).clamp(120, 4096);
        profile.orientation = env::var(ENV_ORIENTATION)
            .ok()
            .map(|value| MobileOrientation::parse(&value))
            .unwrap_or(MobileOrientation::Portrait);
        profile.wifi = env_bool(ENV_WIFI, true);
        profile.cellular = env_bool(ENV_CELLULAR, false);
        profile.low_power = env_bool(ENV_LOW_POWER, false);
        profile
    }

    pub(crate) fn oriented_size(&self) -> (u32, u32) {
        let short = self.width.min(self.height);
        let long = self.width.max(self.height);
        match self.orientation {
            MobileOrientation::Portrait => (short, long),
            MobileOrientation::Landscape => (long, short),
        }
    }

    pub(crate) fn network_type(&self) -> &'static str {
        if self.wifi {
            "wifi"
        } else if self.cellular {
            "cellular"
        } else {
            "offline"
        }
    }

    pub(crate) fn online(&self) -> bool {
        self.wifi || self.cellular
    }
}

pub(crate) fn enabled() -> bool {
    MobileEmulation::from_env().enabled
}

#[allow(dead_code)]
pub(crate) fn apply_env(command: &mut std::process::Command, profile: &MobileEmulation) {
    command
        .env(ENV_ENABLED, if profile.enabled { "1" } else { "0" })
        .env(ENV_WIDTH, profile.width.to_string())
        .env(ENV_HEIGHT, profile.height.to_string())
        .env(ENV_ORIENTATION, profile.orientation.as_str())
        .env(ENV_WIFI, if profile.wifi { "1" } else { "0" })
        .env(ENV_CELLULAR, if profile.cellular { "1" } else { "0" })
        .env(ENV_LOW_POWER, if profile.low_power { "1" } else { "0" });
}

#[allow(dead_code)]
pub(crate) fn set_current_process_env(profile: &MobileEmulation) {
    unsafe {
        env::set_var(ENV_ENABLED, if profile.enabled { "1" } else { "0" });
        env::set_var(ENV_WIDTH, profile.width.to_string());
        env::set_var(ENV_HEIGHT, profile.height.to_string());
        env::set_var(ENV_ORIENTATION, profile.orientation.as_str());
        env::set_var(ENV_WIFI, if profile.wifi { "1" } else { "0" });
        env::set_var(ENV_CELLULAR, if profile.cellular { "1" } else { "0" });
        env::set_var(ENV_LOW_POWER, if profile.low_power { "1" } else { "0" });
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
