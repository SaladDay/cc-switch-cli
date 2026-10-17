//! Route mode weighted selection (smooth weighted round-robin, nginx algorithm)
//!
//! Pure logic, no database access; per-app state lives in [`super::ProviderRouter`]

use std::collections::HashMap;

/// Smooth weighted round-robin state for a single app
#[derive(Default)]
pub(crate) struct SmoothWrrState {
    current: HashMap<String, i64>,
}

impl SmoothWrrState {
    /// Returns the winning member index for this round (ties keep the
    /// earlier list entry) and garbage-collects state for removed members
    pub(crate) fn pick(&mut self, members: &[(String, u32)]) -> usize {
        let total: i64 = members.iter().map(|(_, weight)| *weight as i64).sum();
        let mut best = 0usize;
        let mut best_value = i64::MIN;

        for (index, (id, weight)) in members.iter().enumerate() {
            let value = self.current.entry(id.clone()).or_insert(0);
            *value += *weight as i64;
            if *value > best_value {
                best_value = *value;
                best = index;
            }
        }

        if total > 0 {
            if let Some((winner_id, _)) = members.get(best) {
                if let Some(value) = self.current.get_mut(winner_id) {
                    *value -= total;
                }
            }
        }

        let live: std::collections::HashSet<&str> =
            members.iter().map(|(id, _)| id.as_str()).collect();
        self.current.retain(|id, _| live.contains(id.as_str()));

        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(weights: &[(&str, u32)]) -> Vec<(String, u32)> {
        weights
            .iter()
            .map(|(id, weight)| ((*id).to_string(), *weight))
            .collect()
    }

    fn pick_ids(
        state: &mut SmoothWrrState,
        members: &[(String, u32)],
        picks: usize,
    ) -> Vec<String> {
        (0..picks)
            .map(|_| {
                let index = state.pick(members);
                members[index].0.clone()
            })
            .collect()
    }

    #[test]
    fn three_to_one_repeats_aab_a_cycle() {
        let mut state = SmoothWrrState::default();
        let members = members(&[("a", 3), ("b", 1)]);

        // One full smooth round-robin cycle = sum-of-weights picks
        let picks = pick_ids(&mut state, &members, 4);

        assert_eq!(picks, vec!["a", "a", "b", "a"]);
    }

    #[test]
    fn three_to_one_over_forty_picks_distributes_exactly() {
        let mut state = SmoothWrrState::default();
        let members = members(&[("a", 3), ("b", 1)]);

        let mut counts = (0, 0);
        for _ in 0..40 {
            let index = state.pick(&members);
            if index == 0 {
                counts.0 += 1;
            } else {
                counts.1 += 1;
            }
        }

        assert_eq!(counts, (30, 10));
    }

    #[test]
    fn equal_weights_alternate_in_order() {
        let mut state = SmoothWrrState::default();
        let members = members(&[("a", 1), ("b", 1)]);

        let picks = pick_ids(&mut state, &members, 2);

        assert_eq!(picks, vec!["a", "b"]);
    }

    #[test]
    fn single_member_always_wins() {
        let mut state = SmoothWrrState::default();
        let members = members(&[("solo", 7)]);

        let picks = pick_ids(&mut state, &members, 1);

        assert_eq!(picks, vec!["solo"]);
    }

    #[test]
    fn removed_members_are_garbage_collected() {
        let mut state = SmoothWrrState::default();
        let first = members(&[("a", 1), ("b", 1)]);
        state.pick(&first);

        let second = members(&[("c", 1)]);
        state.pick(&second);

        assert_eq!(state.current.len(), 1);
        assert!(state.current.contains_key("c"));
    }

    #[test]
    fn weight_change_mid_stream_converges() {
        let mut state = SmoothWrrState::default();
        let heavy = members(&[("a", 3), ("b", 1)]);
        for _ in 0..4 {
            state.pick(&heavy);
        }

        let even = members(&[("a", 1), ("b", 1)]);
        let picks = pick_ids(&mut state, &even, 2);

        assert_eq!(picks, vec!["a", "b"]);
    }
}
