// The MIT License (MIT)

// Copyright (c) 2015 Rustcc Developers

// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so,
// subject to the following conditions:

// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
// FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
// COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
// IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
// CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::boxed::FnBox;
use std::cell::UnsafeCell;

use libc;

use context::{Context, Stack};
use context::stack::StackPool;

use runtime::processor::{Processor, WeakProcessor};
use options::Options;

thread_local!(static STACK_POOL: UnsafeCell<StackPool> = UnsafeCell::new(StackPool::new()));

#[derive(Debug)]
pub struct ForceUnwind;

/// Initialization function for make context
extern "C" fn coroutine_initialize(_: usize, f: *mut libc::c_void) -> ! {
    {
        let f = unsafe { Box::from_raw(f as *mut Box<FnBox()>) };

        f();

        // f must be destroyed here or it will cause memory leaks
    }
    Processor::current().unwrap().yield_with(State::Finished);

    unreachable!();
}

pub type Handle = Box<Coroutine>;

/// Coroutine is nothing more than a context and a stack
pub struct Coroutine {
    context: Context,
    stack: Option<Stack>,
    preferred_processor: Option<WeakProcessor>,

    state: State,
}

impl Coroutine {
    fn new(ctx: Context, stack: Option<Stack>) -> Handle {
        Box::new(Coroutine {
            context: ctx,
            stack: stack,
            preferred_processor: None,

            state: State::Initialized,
        })
    }

    pub unsafe fn empty() -> Handle {
        Coroutine::new(Context::empty(), None)
    }

    pub fn spawn_opts(f: Box<FnBox()>, opts: Options) -> Handle {
        let mut stack = STACK_POOL.with(|pool| unsafe {
            (&mut *pool.get()).take_stack(opts.stack_size)
        });

        // NOTE:
        //   We need to use Box<Box<FnBox()>> because Box<FnBox> uses a fat pointer
        //   and is thus 2 pointers wide instead of one, which is why it
        //   can't be transmuted to a single void pointer
        let f = Box::into_raw(Box::new(f)) as *mut libc::c_void;
        let ctx = Context::new(coroutine_initialize, 0, f, &mut stack);

        Coroutine::new(ctx, Some(stack))
    }

    pub fn yield_to(&mut self, state: State, target: &mut Coroutine) {
        self.set_state(state);
        target.set_state(State::Running);
        Context::swap(&mut self.context, &target.context);

        if let State::ForceUnwinding = self.state() {
            panic!(ForceUnwind);
        }
    }

    #[inline]
    fn force_unwind(&mut self) {
        match self.state() {
            State::Initialized | State::Finished => {},
            _ => {
                let mut dummy = unsafe { Coroutine::empty() };
                self.set_state(State::ForceUnwinding);
                Context::swap(&mut dummy.context, &self.context);
            }
        }
    }

    pub fn set_preferred_processor(&mut self, preferred_processor: Option<WeakProcessor>) {
        self.preferred_processor = preferred_processor;
    }

    pub fn preferred_processor(&self) -> Option<Processor> {
        self.preferred_processor.as_ref().and_then(|p| p.upgrade())
    }

    #[inline]
    pub fn state(&self) -> State {
        self.state
    }

    #[inline]
    pub fn set_state(&mut self, state: State) {
        self.state = state
    }
}

impl Drop for Coroutine {
    fn drop(&mut self) {
        self.force_unwind();

        match self.stack.take() {
            None => {}
            Some(st) => {
                STACK_POOL.with(|pool| unsafe {
                    let pool: &mut StackPool = &mut *pool.get();
                    pool.give_stack(st);
                })
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum State {
    Initialized,
    Suspended,
    Running,
    Blocked,
    Finished,
    ForceUnwinding,
}

pub type Result<T> = ::std::result::Result<T, ()>;

/// Sendable coroutine mutable pointer
#[derive(Copy, Clone, Debug)]
pub struct SendableCoroutinePtr(pub *mut Coroutine);
unsafe impl Send for SendableCoroutinePtr {}
