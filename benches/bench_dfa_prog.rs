// Copyright 2015 Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![feature(test)]

extern crate rand;
extern crate regex;
extern crate regex_dfa;
extern crate test;

// Due to macro scoping rules, this definition only applies for the modules
// defined below. Effectively, it allows us to use the same tests for both
// native and dynamic regexes.
macro_rules! regex(
    ($re:expr) => (
        match ::regex_dfa::Dfa::from_regex($re) {
            Ok(re) => re.to_program(),
            Err(err) => panic!("{}", err),
        }
    );
);

type Regex = ::regex_dfa::Program;

mod bench;
