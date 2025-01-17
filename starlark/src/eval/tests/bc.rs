/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Bytecode generation tests.

use crate::{
    assert,
    assert::Assert,
    eval::{bc::opcode::BcOpcode, FrozenDef},
};

fn test_instrs(expected: &[BcOpcode], def_program: &str) {
    let mut a = Assert::new();
    let def = a
        .module("instrs.star", def_program)
        .get("test")
        .unwrap()
        .downcast::<FrozenDef>()
        .unwrap();
    let mut opcodes = def.bc().instrs.opcodes();
    assert_eq!(Some(BcOpcode::EndOfBc), opcodes.pop());
    assert_eq!(expected, opcodes);
}

#[test]
fn test_type() {
    test_instrs(
        &[BcOpcode::LoadLocal, BcOpcode::Type, BcOpcode::Return],
        "def test(x): return type(x)",
    );
}

#[test]
fn test_percent_s_one() {
    test_instrs(
        &[BcOpcode::LoadLocal, BcOpcode::PercentSOne, BcOpcode::Return],
        "def test(x): return '((%s))' % x",
    )
}

#[test]
fn test_format_one() {
    test_instrs(
        &[BcOpcode::LoadLocal, BcOpcode::FormatOne, BcOpcode::Return],
        "def test(x): return '(({}))'.format(x)",
    )
}

#[test]
fn test_percent_s_one_format_one_eval() {
    assert::pass(
        r#"
load("assert.star", "assert")

def test(x):
    return ("<{}>".format(x), "<%s>" % x)

assert.eq(("<1>", "<1>"), test(1))
# Test format does not accidentally call `PercentSOne`.
assert.eq(("<(1,)>", "<1>"), test((1,)))
"#,
    );
}
