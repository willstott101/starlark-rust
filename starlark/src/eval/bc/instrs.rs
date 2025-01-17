/*
 * Copyright 2019 The Starlark in Rust Authors.
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

//! Instructions serialized in byte array.

use std::{
    convert::TryInto,
    fmt,
    fmt::{Display, Formatter},
    mem, ptr, slice,
};

use either::Either;

use crate::{
    codemap::Span,
    eval::bc::{
        addr::{BcAddr, BcAddrOffset, BcPtrAddr},
        instr::BcInstr,
        instr_impl::InstrEndOfBc,
        opcode::{BcOpcode, BcOpcodeHandler},
        repr::{BcInstrRepr, BC_INSTR_ALIGN},
    },
};

impl BcOpcode {
    /// Drop instruction at given address.
    unsafe fn drop_in_place(self, ptr: BcPtrAddr) {
        struct HandlerImpl<'b> {
            ptr: BcPtrAddr<'b>,
        }

        impl BcOpcodeHandler<()> for HandlerImpl<'_> {
            fn handle<I: BcInstr>(self) {
                let HandlerImpl { ptr } = self;
                let instr = ptr.get_instr_mut::<I>();
                unsafe {
                    ptr::drop_in_place(instr);
                }
            }
        }

        self.dispatch(HandlerImpl { ptr });
    }
}

/// Invoke drop for instructions in the buffer.
unsafe fn drop_instrs(instrs: &[usize]) {
    let end = BcPtrAddr::for_slice_end(instrs);
    let mut ptr = BcPtrAddr::for_slice_start(instrs);
    while ptr != end {
        assert!(ptr < end);
        let opcode = ptr.get_opcode();
        opcode.drop_in_place(ptr);
        ptr = ptr.add(opcode.size_of_repr());
    }
}

/// Statically allocate a valid instruction buffer micro-optimization.
///
/// Valid bytecode must end with `EndOfBc` instruction, otherwise evaluation overruns
/// the instruction buffer.
///
/// `BcInstrs` type need to have `Default` (it is convenient).
///
/// Allocating a vec in `BcInstrs::default` is non-free.
///
/// Assertion that `BcInstrs::instrs` is not empty is cheap but not free.
///
/// But if `BcInstrs::instrs` is `Either` allocated instructions or a pointer to statically
/// allocated instructions, then both `BcInstrs::default` is free
/// and evaluation start [is free](https://rust.godbolt.org/z/3nEhWGo4Y).
fn empty_instrs() -> &'static [usize] {
    static END_OF_BC: BcInstrRepr<InstrEndOfBc> = BcInstrRepr::new((BcAddr(0), Vec::new()));
    unsafe {
        slice::from_raw_parts(
            &END_OF_BC as *const BcInstrRepr<_> as *const usize,
            mem::size_of_val(&END_OF_BC) / mem::size_of::<usize>(),
        )
    }
}

pub(crate) struct BcInstrs {
    // We use `usize` here to guarantee the buffer is properly aligned
    // to store `BcInstrLayout`.
    instrs: Either<Box<[usize]>, &'static [usize]>,
}

/// Raw instructions writer.
///
/// Higher level wrapper is `BcWriter`.
pub(crate) struct BcInstrsWriter {
    pub(crate) instrs: Vec<usize>,
}

impl Default for BcInstrs {
    fn default() -> Self {
        BcInstrs {
            instrs: Either::Right(empty_instrs()),
        }
    }
}

impl Drop for BcInstrs {
    fn drop(&mut self) {
        match &self.instrs {
            Either::Left(heap_allocated) => unsafe {
                drop_instrs(heap_allocated);
            },
            Either::Right(_statically_allocated) => {}
        }
    }
}

impl Drop for BcInstrsWriter {
    fn drop(&mut self) {
        unsafe {
            drop_instrs(&self.instrs);
        }
    }
}

pub(crate) struct PatchAddr {
    pub(crate) instr_start: BcAddr,
    pub(crate) arg: BcAddr,
}

impl BcInstrs {
    pub(crate) fn start_ptr(&self) -> BcPtrAddr {
        BcPtrAddr::for_slice_start(&self.instrs)
    }

    pub(crate) fn end(&self) -> BcAddr {
        BcAddr(
            self.instrs
                .len()
                .checked_mul(mem::size_of::<usize>())
                .unwrap()
                .try_into()
                .unwrap(),
        )
    }

    pub(crate) fn end_ptr(&self) -> BcPtrAddr {
        self.start_ptr().offset(self.end())
    }

    #[cfg(test)]
    pub(crate) fn opcodes(&self) -> Vec<BcOpcode> {
        let mut opcodes = Vec::new();
        let end = BcPtrAddr::for_slice_end(&self.instrs);
        let mut ptr = BcPtrAddr::for_slice_start(&self.instrs);
        while ptr != end {
            assert!(ptr < end);
            let opcode = ptr.get_opcode();
            opcodes.push(opcode);
            ptr = ptr.add(opcode.size_of_repr());
        }
        opcodes
    }
}

impl Display for BcInstrs {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut ptr = self.start_ptr();
        loop {
            assert!(ptr < self.end_ptr());
            let ip = ptr.offset_from(self.start_ptr());
            let opcopde = ptr.get_opcode();
            if opcopde == BcOpcode::EndOfBc {
                // We are not printing `EndOfBc`.
                break;
            }
            write!(f, "{}: {:?}", ip.0, opcopde)?;
            opcopde.fmt_append_arg(ptr, f)?;
            write!(f, "; ")?;
            ptr = ptr.add(opcopde.size_of_repr());
        }
        write!(f, "{}: END", ptr.offset_from(self.start_ptr()).0)?;
        Ok(())
    }
}

impl BcInstrsWriter {
    pub(crate) fn new() -> BcInstrsWriter {
        BcInstrsWriter { instrs: Vec::new() }
    }

    fn instrs_len_bytes(&self) -> usize {
        self.instrs
            .len()
            .checked_mul(mem::size_of::<usize>())
            .unwrap()
    }

    pub(crate) fn ip(&self) -> BcAddr {
        BcAddr(self.instrs_len_bytes().try_into().unwrap())
    }

    pub(crate) fn write<I: BcInstr>(&mut self, arg: I::Arg) -> (BcAddr, *const I::Arg) {
        let repr = BcInstrRepr::<I>::new(arg);
        assert!(mem::size_of_val(&repr) % mem::size_of::<usize>() == 0);

        let ip = self.ip();

        let offset_bytes = self.instrs_len_bytes();
        self.instrs.resize(
            self.instrs.len() + mem::size_of_val(&repr) / mem::size_of::<usize>(),
            0,
        );
        unsafe {
            let ptr =
                (self.instrs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut BcInstrRepr<I>;
            ptr::write(ptr, repr);
            (ip, &(*ptr).arg)
        }
    }

    pub(crate) fn addr_to_patch(
        &self,
        (instr_start, addr): (BcAddr, *const BcAddrOffset),
    ) -> PatchAddr {
        unsafe {
            assert_eq!(*addr, BcAddrOffset::FORWARD)
        };
        let offset_bytes =
            unsafe { (addr as *const u8).offset_from(self.instrs.as_ptr() as *const u8) };
        assert!((offset_bytes as usize) < self.instrs_len_bytes());
        PatchAddr {
            instr_start,
            arg: BcAddr(offset_bytes as u32),
        }
    }

    pub(crate) fn patch_addr(&mut self, addr: PatchAddr) {
        unsafe {
            let mem_addr =
                (self.instrs.as_mut_ptr() as *mut u8).add(addr.arg.0 as usize) as *mut BcAddrOffset;
            assert!(*mem_addr == BcAddrOffset::FORWARD);
            *mem_addr = self.ip().offset_from(addr.instr_start);
            debug_assert!(((*mem_addr).0 as usize) % BC_INSTR_ALIGN == 0);
        }
    }

    pub(crate) fn finish(mut self, spans: Vec<(BcAddr, Span)>) -> BcInstrs {
        self.write::<InstrEndOfBc>((self.ip(), spans));
        // We cannot destructure `self` to fetch `instrs` because `Self` has `drop,
        // so we `mem::take`.
        let instrs = mem::take(&mut self.instrs);
        let instrs = instrs.into_boxed_slice();
        assert!((instrs.as_ptr() as usize) % BC_INSTR_ALIGN == 0);
        BcInstrs {
            instrs: Either::Left(instrs),
        }
    }
}

#[cfg(test)]
mod test {
    use std::mem;

    use crate::{
        eval::bc::{
            instr_impl::{InstrConst, InstrPossibleGc, InstrReturn, InstrReturnNone},
            instrs::{BcInstrs, BcInstrsWriter},
        },
        values::FrozenValue,
    };

    #[test]
    fn write() {
        let mut bc = BcInstrsWriter::new();
        bc.write::<InstrReturnNone>(());
        assert_eq!(1, bc.instrs.len());
        bc.write::<InstrPossibleGc>(());
        assert_eq!(2, bc.instrs.len());
    }

    /// Test `BcInstrs::default()` produces something valid.
    #[test]
    fn default() {
        assert_eq!("0: END", BcInstrs::default().to_string());
    }

    #[test]
    fn display() {
        let mut bc = BcInstrsWriter::new();
        bc.write::<InstrConst>(FrozenValue::new_bool(true));
        bc.write::<InstrReturn>(());
        let bc = bc.finish(Vec::new());
        if mem::size_of::<usize>() == 8 {
            assert_eq!("0: Const True; 16: Return; 24: END", format!("{}", bc));
        } else if mem::size_of::<usize>() == 4 {
            // Starlark doesn't work now on 32-bit CPU
        } else {
            panic!("unknown word size: {}", mem::size_of::<usize>());
        }
    }
}
