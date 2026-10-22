use crate::state::State;
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

impl ServiceState {
    /// The state after the first result of a service.
    pub fn first(state: State, max_attempts: u32, now: SystemTime) -> (Self, StateChange) {
        let state_type = if state == State::Ok || max_attempts <= 1 {
            StateType::Hard
        } else {
            StateType::Soft
        };
        let s = ServiceState {
            state,
            state_type,
            attempt: 1,
            last_check: now,
            last_change: now,
            last_hard_change: now,
        };
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
}
