use std;
use std::collections::{BitSet, HashSet, HashMap};
use std::fmt::{Debug, Formatter};
use std::hash::Hash;
use std::mem;
use transition::{SymbRange, TransList};

trait PopArbitrary<T> {
    /// Removes and returns an arbitrary member of this collection.
    ///
    /// If the collection is empty, this panics.
    fn pop_arbitrary(&mut self) -> T;
}

impl<T: Eq + Clone + Hash> PopArbitrary<T> for HashSet<T> {
    fn pop_arbitrary(&mut self) -> T {
        let elt = self.iter().next().unwrap().clone();
        self.remove(&elt);
        elt
    }
}

#[derive(PartialEq, Debug)]
pub struct State {
    pub transitions: TransList,
    pub accepting: bool,
}

impl State {
    pub fn new(accepting: bool) -> State {
        State {
            transitions: TransList::new(),
            accepting: accepting,
        }
    }
}

#[derive(PartialEq)]
pub struct Automaton {
    // TODO: make this private once builder has been transitioned away from
    // using Automaton.
    pub states: Vec<State>,
    pub initial: usize,
}

fn singleton(i: usize) -> BitSet {
    let mut ret = BitSet::with_capacity(i+1);
    ret.insert(i);
    ret
}

impl Debug for Automaton {
    fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
        try!(f.write_fmt(format_args!("Automaton ({} states):\n", self.states.len())));

        for (st_idx, st) in self.states.iter().enumerate() {
            try!(f.write_fmt(format_args!("\tState {} (accepting: {}):\n", st_idx, st.accepting)));

            if !st.transitions.ranges.is_empty() {
                try!(f.write_str("\t\tTransitions:\n"));
                for &(range, target) in &st.transitions.ranges {
                    try!(f.write_fmt(format_args!("\t\t\t{} -- {} => {}\n",
                                                  range.from, range.to, target)));
                }
            }

            if !st.transitions.eps.is_empty() {
                try!(f.write_fmt(format_args!("\t\tEps-transitions: {:?}\n", &st.transitions.eps)));
            }
        }
        Ok(())
    }
}

impl Automaton {
    pub fn new() -> Automaton {
        Automaton {
            states: Vec::new(),
            initial: 0,
        }
    }

    pub fn with_capacity(n: usize) -> Automaton {
        Automaton {
            states: Vec::with_capacity(n),
            initial: 0,
        }
    }

    pub fn add_transition(&mut self, from: usize, to: usize, r: SymbRange) {
        self.states[from].transitions.ranges.push((r, to));
    }

    pub fn add_eps(&mut self, from: usize, to: usize) {
        self.states[from].transitions.eps.push(to);
    }

    /// Creates a deterministic automaton given a non-deterministic one.
    pub fn determinize(&self) -> Automaton {
        let mut ret = Automaton::new();
        let mut state_map = HashMap::<BitSet, usize>::new();
        let mut active_states = Vec::<BitSet>::new();
        let start_state = self.eps_closure(&singleton(0));

        ret.states.push(State::new(self.accepting(&start_state)));
        active_states.push(start_state.clone());
        state_map.insert(start_state, 0);

        while active_states.len() > 0 {
            let state = active_states.pop().unwrap();
            let state_idx = *state_map.get(&state).unwrap();
            let trans = self.transitions(&state);
            for (range, target) in trans.into_iter() {
                let target_idx = if state_map.contains_key(&target) {
                        *state_map.get(&target).unwrap()
                    } else {
                        ret.states.push(State::new(self.accepting(&target)));
                        active_states.push(target.clone());
                        state_map.insert(target, ret.states.len() - 1);
                        ret.states.len() - 1
                    };
                ret.states[state_idx].transitions.ranges.push((range, target_idx));
            }
        }

        ret
    }

    pub fn execute<Iter: Iterator<Item=u32>>(&self, mut iter: Iter) -> bool {
        let mut state = self.initial;

        loop {
            let cur_state = &self.states[state];
            match iter.next() {
                None => return cur_state.accepting,
                Some(ch) => {
                    match cur_state.transitions.find_transition(ch) {
                        Some(next_state) => state = next_state,
                        None => return false,
                    }
                }
            }
        }
    }

    fn accepting_states(&self) -> BitSet {
        let mut ret = BitSet::with_capacity(self.states.len());

        for (idx, state) in self.states.iter().enumerate() {
            if state.accepting {
                ret.insert(idx);
            }
        }

        ret
    }

    fn non_accepting_states(&self) -> BitSet {
        let mut ret = BitSet::with_capacity(self.states.len());

        for (idx, state) in self.states.iter().enumerate() {
            if !state.accepting {
                ret.insert(idx);
            }
        }

        ret
    }

    /// Returns the automaton with all its transitions reversed.
    ///
    /// This may be a non-deterministic automaton. Its states
    /// will have the same indices as those of the original automaton.
    fn reversed(&self) -> Automaton {
        let mut ret = Automaton::with_capacity(self.states.len());

        for st in self.states.iter() {
            ret.states.push(State::new(st.accepting));
        }

        for (idx, st) in self.states.iter().enumerate() {
            for &(ref range, target) in st.transitions.ranges.iter() {
                ret.states[target].transitions.ranges.push((*range, idx));
            }
        }

        ret
    }

    /// Returns an equivalent DFA with a minimal number of states.
    ///
    /// Uses Hopcroft's algorithm.
    pub fn minimize(&self) -> Automaton {
        let acc_states = self.accepting_states();
        let non_acc_states = self.non_accepting_states();
        let mut partition = HashSet::<BitSet>::new();
        let mut distinguishers = HashSet::<BitSet>::new();
        let reversed = self.reversed();

        partition.insert(acc_states.clone());
        if !non_acc_states.is_empty() {
            partition.insert(non_acc_states.clone());
        }
        distinguishers.insert(acc_states.clone());

        while distinguishers.len() > 0 {
            let dist = distinguishers.pop_arbitrary();

            // Find all transitions leading into dist.
            let mut trans = TransList::new();
            for state in dist.iter() {
                trans.ranges.push_all(&reversed.states[state].transitions.ranges[..]);
            }

            let sets = trans.collect_transitions();

            // For each set in our partition so far, split it if
            // some element of `sets` reveals it to contain more than
            // one equivalence class.
            for s in sets.iter() {
                let mut next_partition = HashSet::<BitSet>::new();

                for y in partition.iter() {
                    let y0: BitSet = y.intersection(s).collect();
                    let y1: BitSet = y.difference(s).collect();

                    if y0.is_empty() || y1.is_empty() {
                        next_partition.insert(y.clone());
                    } else {
                        if distinguishers.contains(y) {
                            distinguishers.remove(y);
                            distinguishers.insert(y0.clone());
                            distinguishers.insert(y1.clone());
                        } else if y0.len() < y1.len() {
                            distinguishers.insert(y0.clone());
                        } else {
                            distinguishers.insert(y1.clone());
                        }

                        next_partition.insert(y0);
                        next_partition.insert(y1);
                    }
                }

                partition = next_partition;
            }
        }

        let mut ret = Automaton::new();

        // We need to re-index the states: build a map that maps old indices to
        // new indices.
        let mut old_state_to_new = HashMap::<usize, usize>::new();
        for part in partition.iter() {
            let rep_idx = part.iter().next().unwrap();
            let rep = &self.states[rep_idx];
            ret.states.push(State::new(rep.accepting));

            for state in part.iter() {
                old_state_to_new.insert(state, ret.states.len() - 1);
            }
        }

        // Fix the indices in all transitions to refer to the new state numbering.
        for part in partition.iter() {
            let old_src_idx = part.iter().next().unwrap();
            let new_src_idx = old_state_to_new.get(&old_src_idx).unwrap();

            for &(ref range, old_tgt_idx) in self.states[old_src_idx].transitions.ranges.iter() {
                let new_tgt_idx = old_state_to_new.get(&old_tgt_idx).unwrap();
                ret.states[*new_src_idx].transitions.ranges.push((range.clone(), *new_tgt_idx));
            }

            if part.contains(&self.initial) {
                ret.initial = *new_src_idx;
            }
        }

        ret
    }

    fn eps_closure(&self, states: &BitSet) -> BitSet {
        let mut ret = states.clone();
        let mut new_states = states.clone();
        let mut next_states = BitSet::with_capacity(self.states.len());
        loop {
            for s in &new_states {
                for &t in &self.states[s].transitions.eps {
                    next_states.insert(t);
                }
            }

            if next_states.is_subset(&ret) {
                return ret;
            } else {
                next_states.difference_with(&ret);
                ret.union_with(&next_states);
                mem::swap(&mut next_states, &mut new_states);
                next_states.clear();
            }
        }
    }

    fn accepting(&self, states: &BitSet) -> bool {
        states.iter().any(|s| { self.states[s].accepting })
    }

    /// Finds all the transitions out of the given set of states.
    ///
    /// Only transitions that consume output are returned. In particular, you
    /// probably want `states` to already be eps-closed.
    fn transitions(&self, states: &BitSet) -> Vec<(SymbRange, BitSet)> {
        let trans = states.iter()
                          .flat_map(|s| self.states[s].transitions.ranges.iter().map(|&i| i))
                          .collect();
        let trans = TransList::from_vec(trans).collect_transition_pairs();

        trans.into_iter().map(|x| (x.0, self.eps_closure(&x.1))).collect()
    }
}

#[cfg(test)]
mod tests {
    use automaton::{Automaton, State};
    use builder;
    use regex_syntax;
    use transition::SymbRange;

    fn parse(re: &str) -> Automaton {
        let expr = regex_syntax::Expr::parse(re).unwrap();
        builder::AutomatonBuilder::from_expr(&expr).to_automaton()
    }

    // FIXME: there should be a better way to implement
    // Automaton::execute that doesn't require this convenience function
    fn u32str(s: &str) -> Vec<u32> {
        s.chars().map(|c| c as u32).collect()
    }

    /// Returns an automaton that accepts strings with an even number of 'b's.
    fn even_bs_auto() -> Automaton {
        let mut auto = Automaton::new();

        auto.states.push(State::new(true));
        auto.states.push(State::new(false));

        auto.states[0].transitions.ranges.push((SymbRange::single('a' as u32), 0));
        auto.states[0].transitions.ranges.push((SymbRange::single('b' as u32), 1));
        auto.states[1].transitions.ranges.push((SymbRange::single('a' as u32), 1));
        auto.states[1].transitions.ranges.push((SymbRange::single('b' as u32), 0));

        auto
    }

    #[test]
    fn test_execute() {
        let auto = even_bs_auto();

        assert_eq!(auto.execute(u32str("aaaaaaa").into_iter()), true);
        assert_eq!(auto.execute(u32str("aabaaaaa").into_iter()), false);
        assert_eq!(auto.execute(u32str("aabaaaaab").into_iter()), true);
        assert_eq!(auto.execute(u32str("aabaaaaaba").into_iter()), true);
        assert_eq!(auto.execute(u32str("aabaabaaba").into_iter()), false);
        assert_eq!(auto.execute(u32str("aabbabaaba").into_iter()), true);
    }

    #[test]
    fn test_reverse() {
        let mut auto = even_bs_auto();
        auto.states[0].transitions.ranges.push((SymbRange::single('c' as u32), 1));

        let mut rev = Automaton::new();

        rev.states.push(State::new(true));
        rev.states.push(State::new(false));

        rev.states[0].transitions.ranges.push((SymbRange::single('a' as u32), 0));
        rev.states[0].transitions.ranges.push((SymbRange::single('b' as u32), 1));
        rev.states[1].transitions.ranges.push((SymbRange::single('b' as u32), 0));
        rev.states[1].transitions.ranges.push((SymbRange::single('c' as u32), 0));
        rev.states[1].transitions.ranges.push((SymbRange::single('a' as u32), 1));

        assert_eq!(rev, auto.reversed());
    }

    #[test]
    fn test_minimize() {
        let auto = parse("a*b*").determinize().minimize();

        assert_eq!(auto.execute(u32str("aaabbbbbb").into_iter()), true);
        assert_eq!(auto.execute(u32str("bbbb").into_iter()), true);
        assert_eq!(auto.execute(u32str("a").into_iter()), true);
        assert_eq!(auto.execute(u32str("").into_iter()), true);
        assert_eq!(auto.execute(u32str("ba").into_iter()), false);
        assert_eq!(auto.execute(u32str("aba").into_iter()), false);

        assert_eq!(auto.states.len(), 2);
    }

    #[test]
    fn test_determinize() {
        let auto = parse("a*b*").determinize();

        assert_eq!(auto.execute(u32str("aaabbbbbb").into_iter()), true);
        assert_eq!(auto.execute(u32str("bbbb").into_iter()), true);
        assert_eq!(auto.execute(u32str("a").into_iter()), true);
        assert_eq!(auto.execute(u32str("").into_iter()), true);
        assert_eq!(auto.execute(u32str("ba").into_iter()), false);
        assert_eq!(auto.execute(u32str("aba").into_iter()), false);
    }
}

