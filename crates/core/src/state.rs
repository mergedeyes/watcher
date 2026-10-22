use serde::{Deserialize, Serialize};
use std::fmt;

/// State of a check, following Nagios plugin semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Warning,
    Critical,
    Unknown,
}

impl State {
    /// Maps a plugin exit code to a state. Codes outside `0..=3` are UNKNOWN.
    pub fn from_exit_code(code: i32) -> Self {
        match code {
            0 => State::Ok,
            1 => State::Warning,
            2 => State::Critical,
            _ => State::Unknown,
        }
    }

    /// The Nagios exit code for this state.
    pub fn exit_code(self) -> i32 {
        match self {
            State::Ok => 0,
            State::Warning => 1,
            State::Critical => 2,
            State::Unknown => 3,
        }
    }

    /// Parses `ok`, `warning`/`warn`, `critical`/`crit` or `unknown`, ignoring case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ok" => Some(State::Ok),
            "warning" | "warn" => Some(State::Warning),
            "critical" | "crit" => Some(State::Critical),
            "unknown" => Some(State::Unknown),
            _ => None,
        }
    }

    /// Rank used to pick the worst state: OK < WARNING < UNKNOWN < CRITICAL.
    pub(crate) fn severity(self) -> u8 {
        match self {
            State::Ok => 0,
            State::Warning => 1,
            State::Unknown => 2,
            State::Critical => 3,
        }
    }

    /// Returns the more severe of the two states.
    pub fn worst(self, other: Self) -> Self {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Ok => "OK",
            State::Warning => "WARNING",
            State::Critical => "CRITICAL",
            State::Unknown => "UNKNOWN",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// State of a host, from its host check (Nagios semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostState {
    Up,
    Down,
}

impl HostState {
    /// OK and WARNING mean UP; CRITICAL and UNKNOWN mean DOWN.
    pub fn from_check(state: State) -> Self {
        match state {
            State::Ok | State::Warning => HostState::Up,
            State::Critical | State::Unknown => HostState::Down,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HostState::Up => "UP",
            HostState::Down => "DOWN",
        }
    }
}

impl fmt::Display for HostState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_round_trip() {
        for code in 0..=3 {
            assert_eq!(State::from_exit_code(code).exit_code(), code);
        }
        assert_eq!(State::from_exit_code(127), State::Unknown);
        assert_eq!(State::from_exit_code(-1), State::Unknown);
    }

    #[test]
    fn worst_follows_severity_order() {
        assert_eq!(State::Ok.worst(State::Warning), State::Warning);
        assert_eq!(State::Warning.worst(State::Unknown), State::Unknown);
        assert_eq!(State::Unknown.worst(State::Critical), State::Critical);
        assert_eq!(State::Critical.worst(State::Unknown), State::Critical);
    }

    #[test]
    fn parses_names() {
        assert_eq!(State::parse("CRIT"), Some(State::Critical));
        assert_eq!(State::parse("Warning"), Some(State::Warning));
        assert_eq!(State::parse("bad"), None);
    }
}
