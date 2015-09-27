// Copyright 2015 Joe Neeman.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use ascii_set::AsciiSet;
use bit_set::BitSet;
use char_map::{CharMap, CharMultiMap, CharSet, CharRange};
use error;
use nfa::Nfa;
use searcher::{ExtAsciiSet, ByteSearcher, RepeatUntil, Searcher, SearchThenMatch, StrSearcher};
use std;
use std::ascii::AsciiExt;
use std::collections::{HashSet, HashMap};
use std::fmt::{Debug, Formatter};
use std::hash::Hash;
use std::result::Result;
use transition::Accept;

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

trait SplitSet: Sized {
    /// If this set has a non-trivial intersection with the other set, returns the intersetion and
    /// the difference.
    fn split(&self, other: &Self) -> Option<(Self, Self)>;
}

impl SplitSet for BitSet {
    fn split(&self, other: &BitSet) -> Option<(BitSet, BitSet)> {
        if !self.is_disjoint(other) && !self.is_subset(other) {
            Some((self.intersection(other).collect(), self.difference(other).collect()))
        } else {
            None
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct DfaState {
    pub transitions: CharMap<usize>,
    pub accept: Accept,
}

impl DfaState {
    pub fn new(accept: Accept) -> DfaState {
        DfaState {
            transitions: CharMap::new(),
            accept: accept.clone(),
        }
    }
}

/// Our `Dfa`s are unanchored, in the sense that by default they can match something in the middle
/// of the input string. However, we allow the initial state of the `Dfa` to depend on where we
/// start matching.
#[derive(PartialEq)]
pub struct Dfa {
    states: Vec<DfaState>,

    /// This is the initial state if we start trying to match at the beginning of the string.
    pub init_at_start: Option<usize>,

    /// This gives the initial state if we start trying to match in the middle of the string:
    /// if the previous char in the string matches one of the ranges, we start at the corresponding
    /// state.
    pub init_after_char: CharMap<usize>,

    /// This is the initial state in all other situations.
    pub init_otherwise: Option<usize>
}

impl Dfa {
    /// Returns a `Dfa` with no states.
    pub fn new() -> Dfa {
        Dfa {
            states: Vec::new(),
            init_at_start: None,
            init_after_char: CharMap::new(),
            init_otherwise: None,
        }
    }

    /// Returns the number of states.
    pub fn num_states(&self) -> usize {
        self.states.len()
    }

    /// Creates a `Dfa` from a regex string, bailing out if more than `max_states` states were
    /// required.
    pub fn from_regex_bounded(re: &str, max_states: usize) -> Result<Dfa, error::Error> {
        let mut nfa = try!(Nfa::from_regex(re));
        nfa.remove_predicates();
        let dfa = try!(nfa.determinize(max_states));
        Ok(dfa.minimize())
    }

    pub fn add_state(&mut self, accept: Accept) {
        self.states.push(DfaState::new(accept));
    }

    pub fn add_transition(&mut self, from: usize, to: usize, range: CharRange) {
        self.states[from].transitions.push(range, &to);
    }

    pub fn sort_transitions(&mut self) {
        for st in &mut self.states {
            st.transitions.sort();
        }
    }

    /// Partitions the given states according to what characters they accept.
    fn reject_partition(&self, states: &BitSet) -> Vec<BitSet> {
        if states.is_empty() {
            // Return the empty partition instead of a partition consisting of the empty set.
            return Vec::new();
        }

        // Gets the set of chars rejected from a given state.
        let rejects = |idx: usize| -> CharMap<usize> {
            self.states[idx].transitions.to_char_set().negated().to_char_map(idx)
        };

        // If state `i` rejects char `c` then this will map `c` to `i`.
        let all_rejects = CharMultiMap::from_vec(
            states.iter()
                .flat_map(|idx| rejects(idx).into_iter())
                .collect()
        );

        // This is the collection of sets whose refinement forms the partition we're looking for.
        let sets = all_rejects.group().into_iter().map(|x| x.1);

        // Now build the refinement.
        let mut ret = vec![states.clone()];
        for s in sets {
            let mut next_ret = Vec::new();
            for part in ret {
                if let Some((p1, p2)) = part.split(&s) {
                    next_ret.push(p1);
                    next_ret.push(p2);
                } else {
                    next_ret.push(part);
                }
            }
            ret = next_ret;
        }

        ret
    }

    /// Returns an equivalent DFA with a minimal number of states.
    ///
    /// Uses Hopcroft's algorithm.
    fn minimize(&self) -> Dfa {
        let (never_states, acc_state_partition) = self.accept_partition();
        let reject_partition = self.reject_partition(&never_states);
        let mut partition = Vec::<BitSet>::new();
        let mut distinguishers = HashSet::<BitSet>::new();
        let reversed = self.reversed();

        // This is a little conservative -- we don't actually have to add everything to the set of
        // distinguishers.  But it won't affect the running time much, since the extra
        // distinguishers will just cause a few more no-op loops.
        for state_set in acc_state_partition.into_iter().chain(reject_partition.into_iter()) {
            partition.push(state_set.clone());
            distinguishers.insert(state_set);
        }

        while distinguishers.len() > 0 {
            let dist = distinguishers.pop_arbitrary();
            let sets: Vec<BitSet> = reversed.transitions(&dist)
                                            .into_iter()
                                            .map(|(_, x)| x)
                                            .collect();

            // For each set in our partition so far, split it if
            // some element of `sets` reveals it to contain more than
            // one equivalence class.
            for s in &sets {
                let mut next_partition = Vec::<BitSet>::new();

                for y in partition.iter() {
                    if let Some((y0, y1)) = y.split(s) {
                        if distinguishers.contains(y) {
                            distinguishers.remove(y);
                            distinguishers.insert(y0.clone());
                            distinguishers.insert(y1.clone());
                        } else if y0.len() < y1.len() {
                            distinguishers.insert(y0.clone());
                        } else {
                            distinguishers.insert(y1.clone());
                        }

                        next_partition.push(y0);
                        next_partition.push(y1);
                    } else {
                        next_partition.push(y.clone());
                    }
                }

                partition = next_partition;
            }
        }

        let mut ret = Dfa::new();

        // We need to re-index the states: build a map that maps old indices to
        // new indices.
        let mut old_state_to_new = vec![0; self.states.len()];
        for part in partition.iter() {
            // This unwrap is safe because we don't allow any empty sets into the partition.
            let rep_idx = part.iter().next().unwrap();
            let rep = &self.states[rep_idx];
            ret.states.push(DfaState::new(rep.accept.clone()));

            for state in part.iter() {
                old_state_to_new[state] = ret.states.len() - 1;
            }
        }

        // Fix the indices in all transitions to refer to the new state numbering.
        for part in partition.iter() {
            // This unwrap is safe because we don't allow any empty sets into the partition.
            let old_src_idx = part.iter().next().unwrap();
            let new_src_idx = old_state_to_new[old_src_idx];

            for &(ref range, old_tgt_idx) in self.states[old_src_idx].transitions.iter() {
                let new_tgt_idx = old_state_to_new[old_tgt_idx];
                ret.add_transition(new_src_idx, new_tgt_idx, *range);
            }
        }

        // Fix the initial states to refer to the new numbering.
        if let Some(s) = self.init_at_start {
            ret.init_at_start = Some(old_state_to_new[s])
        }
        if let Some(s) = self.init_otherwise {
            ret.init_otherwise = Some(old_state_to_new[s])
        }
        for &(ref range, state) in self.init_after_char.iter() {
            ret.init_after_char.push(range.clone(), &old_state_to_new[state]);
        }

        ret.normalize_transitions();
        ret
    }

    fn normalize_transitions(&mut self) {
        for st in &mut self.states {
            st.transitions.normalize();
        }
    }

    /// Returns a partition of states according to their accept value. The first tuple element is
    /// the set of states that never accept; the other element is a partition of the remaining
    /// states.
    fn accept_partition(&self) -> (BitSet, Vec<BitSet>) {
        let mut partition = HashMap::<&Accept, BitSet>::new();
        for (idx, st) in self.states.iter().enumerate() {
            partition.entry(&st.accept).or_insert(BitSet::new()).insert(idx);
        }
        let nevers = partition.get(&Accept::never())
                              .map(|x| x.clone())
                              .unwrap_or_else(|| BitSet::new());
        let others = partition.into_iter()
                              .filter(|&(key, _)| !key.is_never())
                              .map(|(_, val)| val)
                              .collect();
        (nevers, others)
    }

    /// Returns the automaton with all its transitions reversed.  Its states will have the same
    /// indices as those of the original automaton.
    ///
    /// Warning: this does not preserve any ending predicates; it's only for reversing the
    /// input-consuming transitions.
    fn reversed(&self) -> Nfa {
        let mut ret = Nfa::with_capacity(self.states.len());

        for st in self.states.iter() {
            ret.add_state(st.accept.clone());
        }

        for (idx, st) in self.states.iter().enumerate() {
            for &(ref range, target) in st.transitions.iter() {
                ret.add_transition(target, idx, *range);
            }
        }

        ret
    }

    pub fn to_program(&self) -> Program {
        let (mut chains, state_map, lengths) = self.chains();
        let map_state = |s| lengths[*state_map.get(&s).unwrap()];

        // Fix up the indices to refer to the new instructions. Note that only the last instruction
        // in each chain can be a Branch, so we only need to look at those.
        for ch in &mut chains {
            if let Some(inst) = ch.last_mut() {
                if let &mut Inst::Branch(ref mut cm) = inst {
                    cm.map_values(&map_state);
                }
            }
        }

        let insts = chains.into_iter().flat_map(|c| c.into_iter()).collect::<Vec<_>>();
        let mut ret = Program::new();
        ret.insts = insts;
        ret.init_after_char = self.init_after_char.clone();
        ret.init_after_char.map_values(&map_state);
        ret.init_at_start = self.init_at_start.map(&map_state);
        ret.init_otherwise = self.init_otherwise.map(&map_state);
        ret
    }

    /// Looks for transitions that only have one possible target state and groups them.
    /// The second return value is a map from the old state index to the element in the first
    /// return value that represents the same state. The third return value is the accumulated
    /// lengths of the chains.
    fn chains(&self) -> (Vec<Vec<Inst>>, HashMap<usize, usize>, Vec<usize>) {
        let mut chains = Vec::<Vec<Inst>>::new();
        let mut map = HashMap::<usize, usize>::new();
        let mut lengths = Vec::new();
        let mut cur_length = 0;
        let rev = self.reversed();

        for st_idx in 0..self.states.len() {
            if self.is_chain_head(st_idx, &rev) {
                let new_chain = self.build_chain(st_idx, &rev);
                map.insert(st_idx, chains.len());
                lengths.push(cur_length);
                cur_length += new_chain.len();
                chains.push(new_chain);
            }
        }

        (chains, map, lengths)
    }

    fn single_target<'a, Iter>(mut iter: Iter) -> Option<usize>
    where Iter: Iterator<Item = &'a (CharRange, usize)> {
        if let Some(&(_, target)) = iter.next() {
            while let Some(&(_, next_target)) = iter.next() {
                if target != next_target {
                    return None;
                }
            }
            Some(target)
        } else {
            None
        }
    }

    fn single_char<'a, Iter>(mut iter: Iter) -> Option<u32>
    where Iter: Iterator<Item = &'a (CharRange, usize)> {
        if let Some(&(range, _)) = iter.next() {
            if range.start == range.end && iter.next().is_none() {
                Some(range.start)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Returns true if this state can be merged into its only target.
    ///
    /// For this to be true, first this state must have only one target state (and that target
    /// cannot be this state itself). Moreover, the target cannot be a starting state, and it must
    /// have only one source state.
    fn is_chain_link(&self, st_idx: usize, reversed: &Nfa) -> bool {
        if let Some(tgt) = Dfa::single_target(self.states[st_idx].transitions.iter()) {
            tgt != st_idx
                && !self.is_starting(tgt)
                && Dfa::single_target(reversed.transitions_from(tgt).iter()).is_some()
        } else {
            false
        }
    }

    fn is_chain_head(&self, st_idx: usize, rev: &Nfa) -> bool {
        // We're at the head of a chain if either we don't have a parent that is a chain link, or
        // if we are a starting state.
        let has_p = if let Some(p) = Dfa::single_target(rev.transitions_from(st_idx).iter()) {
            self.is_chain_link(p, rev)
        } else {
            false
        };
        self.is_starting(st_idx) || !has_p
    }

    fn is_starting(&self, st_idx: usize) -> bool {
        self.init_at_start == Some(st_idx)
            || self.init_otherwise == Some(st_idx)
            || (&self.init_after_char).into_iter().any(|x| x.1 == st_idx)
    }

    fn build_chain(&self, mut st_idx: usize, rev: &Nfa) -> Vec<Inst> {
        let mut ret = Vec::new();
        let mut lit_in_progress = String::new();
        while self.is_chain_link(st_idx, rev) {
            let st = &self.states[st_idx];
            if !st.accept.is_never() {
                ret.push(Inst::Acc(st.accept.clone()));
            }
            if let Some(ch) = Dfa::single_char(st.transitions.iter()) {
                lit_in_progress.push(std::char::from_u32(ch).unwrap());
            } else {
                if !lit_in_progress.is_empty() {
                    ret.push(Inst::Literal(lit_in_progress));
                    lit_in_progress = String::new();
                }
                ret.push(Inst::Char(st.transitions.to_char_set()));
            }

            // This unwrap is OK because self.is_chain_link(st_idx, rev).
            st_idx = Dfa::single_target(self.states[st_idx].transitions.iter()).unwrap();
        }

        if !lit_in_progress.is_empty() {
            ret.push(Inst::Literal(lit_in_progress));
        }

        let st = &self.states[st_idx];
        if !st.accept.is_never() {
            ret.push(Inst::Acc(st.accept.clone()));
        }
        if !st.transitions.is_empty() {
            if ret.is_empty() {
                if let Some((set, transitions)) = loop_optimization(&st.transitions, st_idx) {
                    ret.push(Inst::LoopWhile(set));
                    ret.push(Inst::Branch(transitions));
                } else {
                    ret.push(Inst::Branch(st.transitions.clone()));
                }
            } else {
                ret.push(Inst::Branch(st.transitions.clone()));
            }
        } else if ret.last() != Some(&Inst::Acc(Accept::always())) {
            ret.push(Inst::Reject);
        }

        ret
    }
}

impl Debug for Dfa {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        try!(f.write_fmt(format_args!("Dfa ({} states):\n", self.states.len())));

        try!(f.write_fmt(format_args!("Initial_at_start: {:?}\n", self.init_at_start)));
        try!(f.write_fmt(format_args!("Initial_after_char: {:?}\n", self.init_after_char)));
        try!(f.write_fmt(format_args!("Initial_otherwise: {:?}\n", self.init_otherwise)));

        for (st_idx, st) in self.states.iter().enumerate() {
            try!(f.write_fmt(format_args!("\tState {} (accepting: {:?}):\n", st_idx, st.accept)));

            if !st.transitions.is_empty() {
                try!(f.write_str("\t\tTransitions:\n"));
                for &(range, target) in st.transitions.iter() {
                    try!(f.write_fmt(format_args!("\t\t\t{} -- {} => {}\n",
                                                  range.start, range.end, target)));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    Literal(String),
    Char(CharSet),
    Acc(Accept),
    Branch(CharMap<usize>),

    /// Consumes characters that belong to a set.
    LoopWhile(ExtAsciiSet),
    Reject,
}

/// A deterministic finite automaton, ready for fast searching.
#[derive(Clone)]
pub struct Program {
    insts: Vec<Inst>,
    init_at_start: Option<usize>,
    init_after_char: CharMap<usize>,
    init_otherwise: Option<usize>,
    runner: Box<Runner>,
}

// A Runner is basically a `Fn(&Program, &str) -> Option<(usize, usize)>`,
// except that Box<Fn> isn't clonable, and so we do this as a workaround.
trait Runner {
    fn run(&self, prog: &Program, input: &str) -> Option<(usize, usize)>;
    fn box_clone(&self) -> Box<Runner>;
}

impl Clone for Box<Runner> {
    fn clone(&self) -> Box<Runner> {
        self.box_clone()
    }
}

#[derive(Clone)]
struct FastRunner<S: Searcher> {
    rewind: bool,
    start_state: usize,
    searcher: S,
}

impl<S: Searcher + Clone + 'static> Runner for FastRunner<S> {
    fn run(&self, prog: &Program, s: &str) -> Option<(usize, usize)> {
        for (start, end) in self.searcher.iter(s) {
            let pos = if self.rewind { start } else { end };
            if let Some(match_end) = prog.shortest_match_from(&s[pos..], self.start_state) {
                return Some((start, match_end + pos));
            }
        }
        None
    }

    fn box_clone(&self) -> Box<Runner> {
        Box::new(self.clone())
    }
}

#[derive(Clone)]
struct SlowRunner;

impl Runner for SlowRunner {
    fn run(&self, prog: &Program, s: &str) -> Option<(usize, usize)> {
        if let Some(state) = prog.init_at_start {
            if let Some(end) = prog.shortest_match_from(s, state) {
                return Some((0, end))
            }
        }

        // Skip looping through the string if we know that the match has to start
        // at the beginning.
        if prog.init_otherwise.is_none() && prog.init_after_char.is_empty() {
            return None;
        }

        let mut pos: usize = 0;
        for ch in s.chars() {
            pos += ch.len_utf8();

            if let Some(state) = prog.state_after(ch as u32) {
                if let Some(end) = prog.shortest_match_from(&s[pos..], state) {
                    return Some((pos, pos + end));
                }
            }
        }

        None
    }

    fn box_clone(&self) -> Box<Runner> {
        Box::new(self.clone())
    }
}

// Given the transitions at state index `st_idx`, checks to see if we should insert a `LoopWhile`
// instruction. If so, returns the lookup table and also the remaining transitions.
fn loop_optimization(cm: &CharMap<usize>, st_idx: usize)
-> Option<(ExtAsciiSet, CharMap<usize>)> {
    let loop_cs = cm.filter_values(|st| *st == st_idx).to_char_set();
    if is_common(&loop_cs) && (loop_cs.is_ascii() || loop_cs.contains_non_ascii()) {
        let set = loop_cs.to_ascii_set();
        let set = ExtAsciiSet { set: set, contains_non_ascii: loop_cs.contains_non_ascii() };
        Some((set, cm.filter_values(|st| *st != st_idx)))
    } else {
        None
    }
}

// The `LoopWhile` instruction is an optimization only: if we see a `Branch` instruction for which
// "most" inputs lead back to the same instruction then we will add a `LoopWhile` instruction that
// can be executed efficiently with a `Searcher`. This function determines what counts as "most"
// inputs for this purpose.
fn is_common(cs: &CharSet) -> bool {
    let mut common_chars = CharSet::new();
    common_chars.push(CharRange::new('0' as u32, '9' as u32));
    common_chars.push(CharRange::new('A' as u32, 'Z' as u32));
    common_chars.push(CharRange::new('a' as u32, 'z' as u32));
    let common_chars_count = 10 + 26 + 26;

    cs.intersect(&common_chars).char_count() >= (common_chars_count * 3 / 4)
}

impl Program {
    fn new() -> Program {
        Program {
            insts: Vec::new(),
            init_at_start: None,
            init_after_char: CharMap::new(),
            init_otherwise: None,
            runner: Box::new(SlowRunner),
        }
    }

    pub fn from_regex(re: &str) -> Result<Program, error::Error> {
        Program::from_regex_bounded(re, std::usize::MAX)
    }

    pub fn from_regex_bounded(re: &str, max_states: usize) -> Result<Program, error::Error> {
        let dfa = try!(Dfa::from_regex_bounded(re, max_states));
        let mut prog = dfa.to_program();
        prog.runner = prog.make_runner();
        Ok(prog)
    }

    // On a successful match, returns `Some(end)` where `end` is the index after the end of the
    // match.
    fn shortest_match_from<'a>(&self, mut s: &'a str, mut state: usize) -> Option<usize> {
        use dfa::Inst::*;
        let init_pos = s.as_ptr() as usize;

        loop {
            match self.insts[state] {
                Reject => { return None; },
                Acc(ref a) => {
                    if a.accepts(s.chars().next().map(|c| c as u32)) {
                        return Some(s.as_ptr() as usize - init_pos);
                    }
                    state += 1;
                },
                Char(ref cs) => {
                    if let Some((next_ch, rest)) = s.slice_shift_char() {
                        if cs.contains(next_ch as u32) {
                            state += 1;
                            s = rest;
                            continue;
                        }
                    }
                    return None;
                },
                Literal(ref lit) => {
                    if s.starts_with(lit) {
                        state += 1;
                        s = &s[lit.len()..];
                    } else {
                        return None;
                    }
                },
                Branch(ref cm) => {
                    if let Some((next_ch, rest)) = s.slice_shift_char() {
                        if let Some(&next_state) = cm.get(next_ch as u32) {
                            state = next_state;
                            s = rest;
                            continue;
                        }
                    }
                    return None;
                },
                LoopWhile(ref set) => {
                    let maybe_pos = s.as_bytes().iter().position(|x| !set.contains_byte(*x));
                    if let Some(pos) = maybe_pos {
                        state += 1;
                        s = &s[pos..];
                    } else {
                        return None;
                    }
                },
            }
        }
    }

    // If the set of allowed chars at the given state are all ASCII, build an optimized
    // representation of the allowed chars.
    fn ascii_set(&self, state: usize) -> Option<AsciiSet> {
        match self.insts[state] {
            Inst::Char(ref cs) =>
                if cs.is_ascii() {
                    Some(cs.to_ascii_set())
                } else {
                    None
                },
            Inst::Branch(ref cm) => {
                let cs = cm.to_char_set();
                if cs.is_ascii() {
                    Some(cs.to_ascii_set())
                } else {
                    None
                }
            },
            _ => None,
        }
    }

    /// Returns the index range of the first shortest match, if there is a match. The indices
    /// returned are byte indices of the string. The first index is inclusive; the second is
    /// exclusive, and a little more subtle -- see the crate documentation.
    pub fn shortest_match(&self, s: &str) -> Option<(usize, usize)> {
        self.runner.run(self, s)
    }

    fn make_runner(&self) -> Box<Runner> {
        use dfa::Inst::*;

        fn fast<S>(search: S, state: usize, rewind: bool) -> Box<Runner>
                where S: Searcher + Clone + 'static
        {
            Box::new(FastRunner { searcher: search, start_state: state, rewind: rewind })
        }

        if self.init_after_char.is_empty() && self.init_at_start == self.init_otherwise {
            if let Some(state) = self.init_at_start {
                match self.insts[state] {
                    Literal(ref lit) => {
                        if lit.len() == 1 {
                            let b = lit.as_bytes()[0];
                            if let Some(set) = self.ascii_set(state + 1) {
                                return fast(SearchThenMatch(ByteSearcher(b), set), state, true);
                            } else {
                                return fast(ByteSearcher(b), state + 1, false);
                            }
                        } else {
                            return fast(StrSearcher(lit.clone()), state + 1, false);
                        }
                    },
                    LoopWhile(ref cs) => {
                        return fast(RepeatUntil(cs.complement()), state + 1, false);
                    },
                    Char(ref cs) => {
                        if cs.is_ascii() {
                            return fast(cs.to_ascii_set(), state + 1, false);
                        }
                    },
                    Branch(ref cm) => {
                        let cs = cm.to_char_set();
                        if cs.is_ascii() {
                            return fast(cs.to_ascii_set(), state, true);
                        }
                    },
                    _ => {},
                }
            }
        }

        Box::new(SlowRunner)
    }

    fn state_after(&self, ch: u32) -> Option<usize> {
        self.init_after_char.get(ch).cloned().or(self.init_otherwise)
    }

    /// Checks whether this DFA matches anywhere in the string `s`.
    pub fn is_match(&self, s: &str) -> bool {
        self.shortest_match(s).is_some()
    }
}

impl Debug for Program {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        try!(f.write_fmt(format_args!("Program ({} instructions):\n", self.insts.len())));

        try!(f.write_fmt(format_args!("Initial_at_start: {:?}\n", self.init_at_start)));
        try!(f.write_fmt(format_args!("Initial_after_char: {:?}\n", self.init_after_char)));
        try!(f.write_fmt(format_args!("Initial_otherwise: {:?}\n", self.init_otherwise)));

        for (idx, inst) in self.insts.iter().enumerate() {
            try!(f.write_fmt(format_args!("\tInst {}: {:?}\n", idx, inst)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use char_map::{CharMap, CharRange, CharSet};
    use dfa::{Dfa, Program};
    use nfa::Nfa;
    use std::usize;
    use transition::Accept;

    // Like Dfa::from_regex, but doesn't minimize.
    fn make_dfa(re: &str) -> Dfa {
        let mut nfa = Nfa::from_regex(re).unwrap();
        nfa.remove_predicates();
        nfa.determinize(usize::MAX).unwrap()
    }

    // Returns an automaton that accepts strings with an even number of 'b's.
    fn even_bs_dfa() -> Dfa {
        let mut ret = Dfa::new();

        ret.init_at_start = Some(0);
        ret.add_state(Accept::always());
        ret.add_state(Accept::never());
        ret.add_transition(0, 0, CharRange::single('a' as u32));
        ret.add_transition(0, 1, CharRange::single('b' as u32));
        ret.add_transition(1, 1, CharRange::single('a' as u32));
        ret.add_transition(1, 0, CharRange::single('b' as u32));
        ret
    }

    fn odd_bs_dfa() -> Dfa {
        let mut ret = even_bs_dfa();
        ret.init_at_start = Some(1);
        ret
    }

    #[test]
    fn test_reverse() {
        let dfa = even_bs_dfa();

        let mut rev = Nfa::with_capacity(2);
        rev.add_state(Accept::always());
        rev.add_state(Accept::never());
        rev.add_transition(0, 0, CharRange::single('a' as u32));
        rev.add_transition(0, 1, CharRange::single('b' as u32));
        rev.add_transition(1, 0, CharRange::single('b' as u32));
        rev.add_transition(1, 1, CharRange::single('a' as u32));

        assert_eq!(rev, dfa.reversed());
    }

    #[test]
    fn test_accept_at_end() {
        let re = Program::from_regex("(a*ba*ba*)*$").unwrap();

        assert_eq!(re.shortest_match("aaaaaa"), Some((0, 6)));
        assert_eq!(re.shortest_match("aaaaaba"), Some((6, 7)));
        assert_eq!(re.shortest_match("aaaaaab"), Some((7, 7)));
        assert_eq!(re.shortest_match("baabaaaa"), Some((0, 8)));
        assert_eq!(re.shortest_match("baabaaaab"), Some((1, 9)));
        assert_eq!(re.shortest_match("bbbba"), Some((0, 5)));
    }

    #[test]
    fn test_accept_after_char() {
        // Make a DFA that accepts strings with an odd number of b's, or whose next character
        // is a c.
        let mut dfa = odd_bs_dfa();
        dfa.states[1].accept = Accept { at_eoi: false, at_char: CharSet::single('c' as u32) };
        let prog = dfa.to_program();

        assert_eq!(prog.shortest_match("aaaaaa"), None);
        assert_eq!(prog.shortest_match("aaaaaba"), Some((0, 6)));
        assert_eq!(prog.shortest_match("aaaaaab"), Some((0, 7)));
        assert_eq!(prog.shortest_match("baaaaaa"), Some((0, 1)));
        assert_eq!(prog.shortest_match("aaaaaca"), Some((0, 5)));
        assert_eq!(prog.shortest_match("c"), Some((0, 0)));
        assert_eq!(prog.shortest_match("cbb"), Some((0, 0)));
    }

    #[test]
    fn test_unanchored_start() {
        let mut dfa = odd_bs_dfa();
        dfa.init_at_start = None;
        dfa.init_otherwise = Some(1);
        let prog = dfa.to_program();

        assert_eq!(prog.shortest_match("cacbc"), Some((3, 4)));
        assert_eq!(prog.shortest_match("cacababc"), Some((3, 5)));
        assert_eq!(prog.shortest_match("ab"), Some((1, 2)));
        assert_eq!(prog.shortest_match("cacaaca"), None);
    }

    #[test]
    fn test_start_after() {
        let mut dfa = odd_bs_dfa();
        dfa.init_at_start = None;
        dfa.init_after_char = CharMap::from_vec(vec![(CharRange::single('c' as u32), 1)]);
        let prog = dfa.to_program();

        assert_eq!(prog.shortest_match("baabbababaa"), None);
        assert_eq!(prog.shortest_match("baabbacbabaa"), Some((7, 8)));
        assert_eq!(prog.shortest_match("caaabbababaa"), Some((1, 5)));
    }

    #[test]
    fn test_minimize() {
        let auto = make_dfa("a*b*").minimize();
        assert_eq!(auto.states.len(), 2);

        let auto = make_dfa(r"^a").minimize();
        assert_eq!(auto.states.len(), 2);

        let mut auto = make_dfa("[cgt]gggtaaa|tttaccc[acg]");
        // Since `minimize` is non-deterministic (involving random hashes), run this a bunch of
        // times.
        for _ in 0..100 {
            auto = auto.minimize();
            assert_eq!(auto.states.len(), 16);
        }
    }

   #[test]
    fn test_class_normalized() {
        let re = make_dfa("[abcdw]");
        assert_eq!(re.states.len(), 2);
        // The order of the states is arbitrary, but one should have two transitions and
        // the other should have zero.
        assert_eq!(re.states[0].transitions.len() + re.states[1].transitions.len(), 2);
    }

    #[test]
    fn test_word_boundary() {
        let re = Program::from_regex(r"\btest\b").unwrap();
        assert_eq!(re.shortest_match("This is a test."), Some((10, 14)));
        let re = Program::from_regex(r"\bהחומוס\b").unwrap();
        assert_eq!(re.shortest_match("למי יש את החומוס הכי טוב בארץ?"), Some((17, 29)));
    }

    #[test]
    fn test_max_states() {
        assert!(Program::from_regex_bounded("foo", 3).is_err());
        assert!(Program::from_regex_bounded("foo", 4).is_ok());
    }

    #[test]
    fn test_adjacent_predicates() {
        assert!(Program::from_regex(r"\btest\b\B").unwrap().insts.is_empty());
        assert!(Program::from_regex(r"\btest\B\b").unwrap().insts.is_empty());
        assert!(Program::from_regex(r"test1\b\Btest2").unwrap().insts.is_empty());

        let re = Program::from_regex(r"\b\btest\b\b").unwrap();
        assert_eq!(re.shortest_match("This is a test."), Some((10, 14)));
        assert_eq!(re.shortest_match("This is a test"), Some((10, 14)));
        assert_eq!(re.shortest_match("test"), Some((0, 4)));

        let re = Program::from_regex(r"(\btest\b *)+end").unwrap();
        assert_eq!(re.shortest_match("This is a test test test end."), Some((10, 28)));
    }

    #[test]
    fn test_syntax_error() {
        assert!(Program::from_regex("(abc").is_err());
    }

    #[test]
    fn test_multi_line() {
        let re = Program::from_regex(r"^A line.$").unwrap();
        assert_eq!(re.shortest_match("Line 1\nA line.\nLine 2\n"), None);

        let re = Program::from_regex(r"(?m)^A line.$").unwrap();
        assert_eq!(re.shortest_match("Line 1\nA line.\nLine 2\n"), Some((7, 14)));
    }

    #[test]
    fn test_dot_matches_nl() {
        let re = Program::from_regex(r"a.b").unwrap();
        assert_eq!(re.shortest_match("a\nb"), None);
        assert_eq!(re.shortest_match("a\rb"), None);

        let re = Program::from_regex(r"(?s)a.b").unwrap();
        assert_eq!(re.shortest_match("a\nb"), Some((0, 3)));
        assert_eq!(re.shortest_match("a\rb"), Some((0, 3)));
    }

    #[test]
    fn test_bug() {
        let re = Program::from_regex("(.*)c(.*)").unwrap();
        let text = "abcde";
        println!("{:?}", re);
        assert_eq!(re.shortest_match(text), Some((0, 3)));
    }
}

