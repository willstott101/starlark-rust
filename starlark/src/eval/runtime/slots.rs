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

use std::{cell::Cell, convert::TryInto, mem};

use gazebo::prelude::*;

use crate as starlark;
use crate::values::{Trace, Value};

#[derive(Clone, Copy, Dupe, Debug, PartialEq, Eq)]
pub(crate) struct LocalSlotId(pub(crate) u32);

impl LocalSlotId {
    pub fn new(index: u32) -> Self {
        Self(index)
    }
}

#[derive(Clone, Copy, Dupe, Debug, PartialEq, Eq)]
pub(crate) struct LocalSlotBase(u32);

/// Slots that are used in a local context, e.g. for a function that is executing.
/// Always mutable, never frozen. Uses the `ValueRef` because they have reference
/// semantics - if a variable gets mutated, someone who has a copy will see the
/// mutation.
///
/// The slots are stored as a linear buffer. To make a function call we:
///
/// 1. `reserve` some slots at the end, which will be the locals for the callee.
/// 2. Fill up these slots with parameters.
/// 3. `utilise` these slots by moving the register index to these slots.
/// 4. Execute the function.
/// 5. `release` these slots by moving the register index back.
#[derive(Trace)]
pub(crate) struct LocalSlots<'v> {
    // All the slots are stored continguously
    slots: Vec<Cell<Option<Value<'v>>>>,
    // The current index at which LocalSlotId is relative to
    #[trace(unsafe_ignore)]
    base: LocalSlotBase,
}

impl<'v> LocalSlots<'v> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            base: LocalSlotBase(0),
        }
    }

    pub fn reserve(&mut self, len: u32) -> LocalSlotBase {
        let res = LocalSlotBase(self.slots.len().try_into().unwrap());
        self.slots
            .resize(self.slots.len() + len as usize, Cell::new(None));
        res
    }

    pub fn utilise(&mut self, base: LocalSlotBase) -> LocalSlotBase {
        mem::replace(&mut self.base, base)
    }

    pub fn release_after(&mut self, base: LocalSlotBase) {
        // If people create two reservations and use them in an odd manner, we probably get issues here
        // but they will be caught by the bound check.
        // NOTE: If we ever remove bounds checks, this probably needs to check its the final reservation.
        self.slots.truncate(base.0 as usize);
    }

    pub fn release(&mut self, new_base: LocalSlotBase) {
        self.release_after(self.base);
        self.base = new_base;
    }

    pub fn get_slots_at(&self, base: LocalSlotBase) -> &[Cell<Option<Value<'v>>>] {
        &self.slots[base.0 as usize..]
    }

    /// Gets a local variable. Returns None to indicate the variable is not yet assigned.
    #[inline(always)]
    pub fn get_slot(&self, slot: LocalSlotId) -> Option<Value<'v>> {
        self.slots[self.base.0 as usize + slot.0 as usize].get()
    }

    pub fn set_slot(&self, slot: LocalSlotId, value: Value<'v>) {
        self.slots[self.base.0 as usize + slot.0 as usize].set(Some(value));
    }
}
