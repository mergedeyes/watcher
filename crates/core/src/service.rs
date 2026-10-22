use crate::state::{HostState, State};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Whether a state is confirmed (Nagios semantics).
///
/// A problem starts SOFT and becomes HARD after `max_attempts` consecutive
/// non-OK results. Only hard changes are meant to notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateType {
    Soft,
    Hard,
}

/// The tracked state of a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceState {
    pub state: State,
    pub state_type: StateType,
    /// Current attempt, `1..=max_attempts`. Stays at 1 while OK.
    pub attempt: u32,
    pub last_check: SystemTime,
    /// When `state` last changed (soft or hard).
    pub last_change: SystemTime,
    /// When the state last changed as a hard change.
    pub last_hard_change: SystemTime,
    /// Recent results, for flap detection.
    #[serde(default)]
    pub history: FlapHistory,
    /// The state changes too often; see [`ServiceState::detect_flapping`].
    #[serde(default)]
    pub flapping: bool,
}

/// How many results flap detection looks at (Nagios: 21, so 20 possible
/// changes).
pub const FLAP_WINDOW: u8 = 21;

/// The last [`FLAP_WINDOW`] states, 2 bits each, newest in the lowest bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FlapHistory {
    pub bits: u64,
    pub len: u8,
}

impl FlapHistory {
    pub fn push(&mut self, state: State) {
        let mask = (1u64 << (2 * FLAP_WINDOW)) - 1;
        self.bits = ((self.bits << 2) | state.exit_code() as u64) & mask;
        self.len = (self.len + 1).min(FLAP_WINDOW);
    }

    /// The `i`-th newest state's code.
    fn get(&self, i: u8) -> u64 {
        (self.bits >> (2 * i)) & 3
    }

    /// Percent state change over the window, 0 to 100. As in Nagios,
    /// recent changes weigh more: from 0.8 for the oldest possible change
    /// to 1.2 for the newest.
    pub fn percent_change(&self) -> f64 {
        let changes = f64::from(FLAP_WINDOW - 1);
        let mut total = 0.0;
        for i in 1..self.len {
            if self.get(i) != self.get(i - 1) {
                // 0 for the oldest change in a full window, 19 for the newest.
                let age = f64::from(FLAP_WINDOW - 1 - i);
                total += 0.8 + age * 0.4 / (changes - 1.0);
            }
        }
        total * 100.0 / changes
    }
}

/// Percent state change that starts (`high`) and ends (`low`) flapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlapThresholds {
    pub low: f64,
    pub high: f64,
}

impl FlapThresholds {
    /// The Nagios defaults for services.
    pub const DEFAULT: FlapThresholds = FlapThresholds {
        low: 5.0,
        high: 20.0,
    };
}

/// A state change caused by a check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateChange {
    /// `None` for the first result of a service.
    pub from: Option<(State, StateType)>,
    pub to: (State, StateType),
    /// A hard change: a confirmed problem, a change between problem
    /// states, or a recovery from a hard problem.
    pub hard: bool,
}

/// A change of a host's UP/DOWN state, caused by its host check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostChange {
    pub from: Option<(HostState, StateType)>,
    pub to: (HostState, StateType),
    /// A confirmed DOWN, or a recovery from a confirmed DOWN.
    pub hard: bool,
}

impl StateChange {
    /// The host's change if this is a change of its host check: `None`
    /// when UP/DOWN and the state type stay the same (CRITICAL -> UNKNOWN,
    /// WARNING -> OK). UP is always HARD.
    pub fn host(&self) -> Option<HostChange> {
        let host = |(s, t): (State, StateType)| match HostState::from_check(s) {
            HostState::Up => (HostState::Up, StateType::Hard),
            HostState::Down => (HostState::Down, t),
        };
        let from = self.from.map(host);
        let to = host(self.to);
        if from == Some(to) {
            return None;
        }
        // The hard state before: soft problems start from a hard UP.
        let before = self.from.map(|(s, t)| match t {
            StateType::Soft => HostState::Up,
            StateType::Hard => HostState::from_check(s),
        });
        Some(HostChange {
            from,
            to,
            hard: self.hard && before != Some(to.0),
        })
    }
}

impl ServiceState {
    /// The state after the first result of a service.
    pub fn first(state: State, max_attempts: u32, now: SystemTime) -> (Self, StateChange) {
        let state_type = if state == State::Ok || max_attempts <= 1 {
            StateType::Hard
        } else {
            StateType::Soft
        };
        let mut s = ServiceState {
            state,
            state_type,
            attempt: 1,
            last_check: now,
            last_change: now,
            last_hard_change: now,
            history: FlapHistory::default(),
            flapping: false,
        };
        s.history.push(state);
        let change = StateChange {
            from: None,
            to: (state, state_type),
            hard: state_type == StateType::Hard,
        };
        (s, change)
    }

    /// Applies a check result and returns the change, if the state or state
    /// type changed.
    pub fn apply(
        &mut self,
        state: State,
        max_attempts: u32,
        now: SystemTime,
    ) -> Option<StateChange> {
        let before = (self.state, self.state_type);
        self.last_check = now;
        self.history.push(state);

        let hard = match (self.state_type, state) {
            // Recovery. From a hard problem this is a hard change; from a soft
            // one it just ends the retries.
            (_, State::Ok) => {
                let hard = self.state != State::Ok && self.state_type == StateType::Hard;
                self.state_type = StateType::Hard;
                self.attempt = 1;
                hard
            }
            // A confirmed problem stays confirmed; changing between problem
            // states (WARNING -> CRITICAL) is a hard change.
            (StateType::Hard, _) if self.state != State::Ok => self.state != state,
            // A new problem, or another failed attempt.
            _ => {
                self.attempt = if self.state == State::Ok {
                    1
                } else {
                    self.attempt + 1
                };
                if self.attempt >= max_attempts.max(1) {
                    self.attempt = max_attempts.max(1);
                    self.state_type = StateType::Hard;
                    true
                } else {
                    self.state_type = StateType::Soft;
                    false
                }
            }
        };

        self.state = state;
        if self.state != before.0 {
            self.last_change = now;
        }
        if hard {
            self.last_hard_change = now;
        }
        let after = (self.state, self.state_type);
        (after != before).then_some(StateChange {
            from: Some(before),
            to: after,
            hard,
        })
    }

    /// Updates [`flapping`](Self::flapping) after a result: it starts when the
    /// percent state change reaches `high` and ends below `low`. Returns
    /// `Some(true)` when flapping starts, `Some(false)` when it ends.
    /// `None` thresholds turn detection off.
    pub fn detect_flapping(&mut self, thresholds: Option<FlapThresholds>) -> Option<bool> {
        let flapping = match thresholds {
            None => false,
            Some(t) => {
                let percent = self.history.percent_change();
                if self.flapping {
                    percent >= t.low
                } else {
                    percent >= t.high
                }
            }
        };
        (flapping != self.flapping).then(|| {
            self.flapping = flapping;
            flapping
        })
    }

    /// Whether the next check should use the retry interval.
    pub fn retrying(&self) -> bool {
        self.state != State::Ok && self.state_type == StateType::Soft
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use State::*;
    use StateType::*;

    const T: SystemTime = SystemTime::UNIX_EPOCH;

    fn run(results: &[State], max_attempts: u32) -> Vec<(State, StateType, u32, Option<bool>)> {
        let (mut s, change) = ServiceState::first(results[0], max_attempts, T);
        let mut out = vec![(s.state, s.state_type, s.attempt, Some(change.hard))];
        for &r in &results[1..] {
            let change = s.apply(r, max_attempts, T);
            out.push((s.state, s.state_type, s.attempt, change.map(|c| c.hard)));
        }
        out
    }

    #[test]
    fn problem_becomes_hard_after_max_attempts() {
        assert_eq!(
            run(&[Ok, Critical, Critical, Critical, Critical, Ok], 3),
            [
                (Ok, Hard, 1, Some(true)),
                (Critical, Soft, 1, Some(false)),
                (Critical, Soft, 2, None),
                (Critical, Hard, 3, Some(true)),
                (Critical, Hard, 3, None),
                (Ok, Hard, 1, Some(true)),
            ]
        );
    }

    #[test]
    fn soft_recovery_is_not_hard() {
        assert_eq!(
            run(&[Ok, Warning, Ok], 3),
            [
                (Ok, Hard, 1, Some(true)),
                (Warning, Soft, 1, Some(false)),
                (Ok, Hard, 1, Some(false))
            ]
        );
    }

    #[test]
    fn soft_state_can_change_and_hard_problem_changes_are_hard() {
        assert_eq!(
            run(&[Ok, Warning, Critical, Critical, Warning, Warning], 3),
            [
                (Ok, Hard, 1, Some(true)),
                (Warning, Soft, 1, Some(false)),
                (Critical, Soft, 2, Some(false)),
                (Critical, Hard, 3, Some(true)),
                (Warning, Hard, 3, Some(true)),
                (Warning, Hard, 3, None),
            ]
        );
    }

    #[test]
    fn max_attempts_one_is_immediately_hard() {
        assert_eq!(
            run(&[Critical, Ok, Unknown], 1),
            [
                (Critical, Hard, 1, Some(true)),
                (Ok, Hard, 1, Some(true)),
                (Unknown, Hard, 1, Some(true))
            ]
        );
    }

    #[test]
    fn first_result_problem_is_soft() {
        let (s, change) = ServiceState::first(Critical, 3, T);
        assert_eq!((s.state_type, change.hard), (Soft, false));
        assert!(s.retrying());
    }

    fn history(states: &[State]) -> FlapHistory {
        let mut h = FlapHistory::default();
        for &s in states {
            h.push(s);
        }
        h
    }

    #[test]
    fn percent_change_weighs_recent_changes_more() {
        assert_eq!(history(&[Ok; 30]).percent_change(), 0.0);
        // Every result differs: all 20 changes, weights 0.8..=1.2 average 1.
        let alternating: Vec<State> = (0..21)
            .map(|i| if i % 2 == 0 { Ok } else { Critical })
            .collect();
        assert!((history(&alternating).percent_change() - 100.0).abs() < 1e-9);
        // One change, newest vs. oldest position in a full window.
        let mut newest = vec![Ok; 20];
        newest.push(Critical);
        let mut oldest = vec![Ok];
        oldest.extend([Critical; 20]);
        assert!((history(&newest).percent_change() - 6.0).abs() < 1e-9);
        assert!((history(&oldest).percent_change() - 4.0).abs() < 1e-9);
        // Older results fall out of the window.
        let mut gone = oldest.clone();
        gone.push(Critical);
        assert_eq!(history(&gone).percent_change(), 0.0);
    }

    #[test]
    fn flapping_starts_high_and_ends_low() {
        let t = Some(FlapThresholds::DEFAULT);
        let (mut s, _) = ServiceState::first(Ok, 1, T);
        let mut started = None;
        for (i, state) in [Critical, Ok, Critical, Ok, Critical]
            .into_iter()
            .enumerate()
        {
            s.apply(state, 1, T);
            if s.detect_flapping(t) == Some(true) {
                started = Some(i);
            }
        }
        // 4 changes of weight ~1.2 = 24% >= 20%; 3 are 18%.
        assert_eq!(started, Some(3));
        assert!(s.flapping);
        let mut ended = None;
        for i in 0..21 {
            s.apply(Critical, 1, T);
            if s.detect_flapping(t) == Some(false) {
                ended = Some(i);
                break;
            }
        }
        assert!(ended.is_some());
        assert!(s.history.percent_change() < 5.0);
        // Turning detection off ends flapping.
        s.flapping = true;
        assert_eq!(s.detect_flapping(None), Some(false));
        assert_eq!(s.detect_flapping(None), None);
    }

    #[test]
    fn host_changes_follow_up_down() {
        use HostState::{Down, Up};
        let mut changes = Vec::new();
        let (mut s, first) = ServiceState::first(Ok, 2, T);
        changes.push(first.host());
        for r in [Warning, Ok, Critical, Unknown, Unknown, Ok, Critical, Ok] {
            changes.push(s.apply(r, 2, T).and_then(|c| c.host()));
        }
        let change = |from, to, hard| Some(HostChange { from, to, hard });
        assert_eq!(
            changes,
            [
                change(None, (Up, Hard), true),
                // WARNING is still UP.
                None,
                None,
                change(Some((Up, Hard)), (Down, Soft), false),
                change(Some((Down, Soft)), (Down, Hard), true),
                // CRITICAL -> UNKNOWN: DOWN either way.
                None,
                change(Some((Down, Hard)), (Up, Hard), true),
                change(Some((Up, Hard)), (Down, Soft), false),
                // A soft DOWN that ends isn't a hard change.
                change(Some((Down, Soft)), (Up, Hard), false),
            ]
        );
    }
}
