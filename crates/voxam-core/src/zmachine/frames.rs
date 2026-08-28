//! The routine call state: frames, locals, and the stack (§6).
//!
//! The "state of play" includes the chain of routines that have
//! called each other (§6.1). Each link is a frame holding the
//! caller's return address, where the result should go, the
//! routine's local variables, and its own portion of the stack --
//! because a routine starts with an empty stack and can never reach
//! past its own frame (§6.3.1, §6.3.2).

use crate::errors::VoxamError;
use crate::zmachine::routine::Routine;
use crate::zmachine::snapshot::FrameSnapshot;

/// Local variables are numbered 1 to 15 (§4.2.2).
const FIRST_LOCAL: u8 = 1;

/// §6.3.3 defines a call's "usage" as 4 plus its local count, and
/// sets 1024 total usage as the classic floor a game may assume --
/// while noting later games need much more. This ceiling is 64
/// times that floor: far above any legitimate game, and low enough
/// to turn runaway recursion into a loud halt rather than a silent
/// hang. (Zork 1 release 15 has exactly such a bug, which crashed
/// period interpreters by exhausting their fixed stacks.)
const FRAME_USAGE: usize = 4;
const USAGE_LIMIT: usize = 64 * 1024;

/// One routine invocation's private state (§6.1). Deliberately
/// mutable: locals and the stack change as the routine runs.
struct Frame {
    /// Where execution resumes when this routine returns (§6.4).
    return_address: usize,
    /// The caller's variable for the return value, or `None` when
    /// the result is thrown away (§6.4.1).
    store_variable: Option<u8>,
    /// The routine's local variable values, in order (§6.4.4).
    locals: Vec<u16>,
    /// How many arguments the caller supplied, which
    /// check_arg_count can ask about (§6.4.4.1).
    argument_count: usize,
    /// This routine's portion of the stack, empty at routine start
    /// (§6.3.1).
    stack: Vec<u16>,
}

/// What a popped frame directs next: where to resume, and where the
/// result goes (§6.4).
#[derive(Debug)]
pub struct ReturnDirection {
    pub return_address: usize,
    pub store_variable: Option<u8>,
}

/// The chain of routine invocations (§6.1).
///
/// Born holding one base frame: outside Version 6, execution begins
/// at an instruction that is not inside any called routine (§5.5),
/// and that resting place needs a stack to work with too.
pub struct CallStack {
    frames: Vec<Frame>,
    usage: usize,
}

impl Default for CallStack {
    fn default() -> Self {
        Self::new()
    }
}

impl CallStack {
    /// Start the call state with an un-returnable base frame.
    pub fn new() -> Self {
        Self {
            frames: vec![Frame {
                return_address: 0,
                store_variable: None,
                locals: Vec::new(),
                argument_count: 0,
                stack: Vec::new(),
            }],
            usage: 0,
        }
    }

    /// How many frames deep the call state is, counting the base.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// How many arguments the current routine received (§6.4.4.1).
    pub fn argument_count(&self) -> usize {
        self.top().argument_count
    }

    /// Enter a routine, creating its frame (§6.4).
    ///
    /// Locals begin at the routine header's initial values -- zero
    /// from Version 5 -- and the arguments then overwrite the first
    /// locals (§6.4.4). Spare arguments are thrown away (§6.4.4.1).
    /// Fails if the call would push total stack usage past the
    /// ceiling (§6.3.3), which is almost certainly runaway
    /// recursion.
    pub fn call(
        &mut self,
        routine: &Routine,
        arguments: &[u16],
        return_address: usize,
        store_variable: Option<u8>,
    ) -> Result<(), VoxamError> {
        let mut initial = routine.initial_locals.clone();

        self.claim_usage(FRAME_USAGE + initial.len())?;

        let claimed = arguments.len().min(initial.len());
        initial[..claimed].copy_from_slice(&arguments[..claimed]);

        self.frames.push(Frame {
            return_address,
            store_variable,
            locals: initial,
            argument_count: arguments.len(),
            stack: Vec::new(),
        });

        Ok(())
    }

    /// Leave the current routine, discarding its state (§6.3.2) and
    /// returning where execution resumes and where the result goes.
    /// Fails when only the base frame remains: there is no routine
    /// to return from.
    pub fn pop_frame(&mut self) -> Result<ReturnDirection, VoxamError> {
        if self.frames.len() == 1 {
            return Err(stack_error(
                "cannot return: no routine has been called (§6.4)".into(),
            ));
        }

        let frame = self.frames.pop().expect("more than the base frame");
        self.usage -= FRAME_USAGE + frame.locals.len() + frame.stack.len();

        Ok(ReturnDirection {
            return_address: frame.return_address,
            store_variable: frame.store_variable,
        })
    }

    /// Capture the whole call chain as immutable frames (§6.1):
    /// every frame from the base up, frozen.
    pub fn snapshot(&self) -> Vec<FrameSnapshot> {
        self.frames
            .iter()
            .map(|frame| FrameSnapshot {
                return_address: frame.return_address,
                store_variable: frame.store_variable,
                locals: frame.locals.clone(),
                argument_count: frame.argument_count,
                stack: frame.stack.clone(),
            })
            .collect()
    }

    /// Write a captured call chain back over the live one (§6.1.2).
    ///
    /// The frames become the entire call state; usage is recomputed
    /// from scratch, since nothing of the abandoned state survives.
    /// Fails on an empty chain -- even a game at rest stands on the
    /// base frame (§5.5) -- or one whose usage would pass the
    /// §6.3.3 ceiling, which no honest capture of this machine can
    /// reach.
    pub fn restore(&mut self, frames: &[FrameSnapshot]) -> Result<(), VoxamError> {
        if frames.is_empty() {
            return Err(stack_error(
                "cannot restore an empty call chain: the base frame always exists \
                 (§5.5)"
                    .into(),
            ));
        }

        let usage: usize = frames.iter().map(|frame| frame.stack.len()).sum::<usize>()
            + frames[1..]
                .iter()
                .map(|frame| FRAME_USAGE + frame.locals.len())
                .sum::<usize>();

        if usage > USAGE_LIMIT {
            return Err(stack_error(format!(
                "restoring this call chain would put stack usage past {USAGE_LIMIT} \
                 (§6.3.3): it cannot be an honest capture"
            )));
        }

        self.frames = frames
            .iter()
            .map(|frame| Frame {
                return_address: frame.return_address,
                store_variable: frame.store_variable,
                locals: frame.locals.clone(),
                argument_count: frame.argument_count,
                stack: frame.stack.clone(),
            })
            .collect();
        self.usage = usage;

        Ok(())
    }

    /// Read a local variable of the current routine (§4.2.2).
    pub fn local(&self, number: u8) -> Result<u16, VoxamError> {
        let index = self.local_index(number)?;

        Ok(self.top().locals[index])
    }

    /// Write a local variable of the current routine (§4.2.2).
    pub fn set_local(&mut self, number: u8, value: u16) -> Result<(), VoxamError> {
        let index = self.local_index(number)?;
        self.top_mut().locals[index] = value;

        Ok(())
    }

    /// Push a word onto the current routine's stack (§6.3), unless
    /// the push would pass the usage ceiling (§6.3.3).
    pub fn push(&mut self, value: u16) -> Result<(), VoxamError> {
        self.claim_usage(1)?;
        self.top_mut().stack.push(value);

        Ok(())
    }

    /// Pull the top word off the current routine's stack (§6.3); a
    /// routine cannot reach past its own frame (§6.3.1).
    pub fn pop(&mut self) -> Result<u16, VoxamError> {
        match self.top_mut().stack.pop() {
            Some(value) => {
                self.usage -= 1;

                Ok(value)
            }
            None => Err(stack_error(
                "cannot pull from an empty stack: a routine only sees values it \
                 pushed itself (§6.3.1)"
                    .into(),
            )),
        }
    }

    /// Read the top of the stack without pulling it (§6.3.4).
    ///
    /// The seven indirect-reference opcodes read the stack top in
    /// place, leaving the depth unchanged.
    pub fn peek(&self) -> Result<u16, VoxamError> {
        match self.top().stack.last() {
            Some(value) => Ok(*value),
            None => Err(stack_error(
                "cannot read the top of an empty stack in place (§6.3.4)".into(),
            )),
        }
    }

    /// Overwrite the top of the stack in place (§6.3.4).
    pub fn replace_top(&mut self, value: u16) -> Result<(), VoxamError> {
        match self.top_mut().stack.last_mut() {
            Some(top) => {
                *top = value;

                Ok(())
            }
            None => Err(stack_error(
                "cannot overwrite the top of an empty stack in place (§6.3.4)".into(),
            )),
        }
    }

    fn top(&self) -> &Frame {
        self.frames.last().expect("the base frame always exists")
    }

    fn top_mut(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("the base frame always exists")
    }

    /// Map a local number to its list index, policing §4.2.2.
    fn local_index(&self, number: u8) -> Result<usize, VoxamError> {
        let count = self.top().locals.len();

        if number < FIRST_LOCAL || usize::from(number) > count {
            return Err(stack_error(format!(
                "the current routine has {count} locals, so local {number} does not \
                 exist (§4.2.2)"
            )));
        }

        Ok(usize::from(number - FIRST_LOCAL))
    }

    /// Charge stack usage against the §6.3.3 ceiling.
    ///
    /// A period interpreter's fixed stack would overflow and crash
    /// here; halting loudly names the almost-certain culprit
    /// instead of hanging forever on an unbounded one.
    fn claim_usage(&mut self, amount: usize) -> Result<(), VoxamError> {
        if self.usage + amount > USAGE_LIMIT {
            return Err(stack_error(format!(
                "stack usage passed {USAGE_LIMIT} (§6.3.3 promises games far less): \
                 almost certainly runaway recursion"
            )));
        }

        self.usage += amount;

        Ok(())
    }
}

fn stack_error(message: String) -> VoxamError {
    VoxamError::ZMachineStack(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(initial_locals: &[u16]) -> Routine {
        Routine {
            address: 0x100,
            initial_locals: initial_locals.to_vec(),
            first_instruction: 0x101,
        }
    }

    #[test]
    fn starts_with_an_unreturnable_base_frame() {
        let mut calls = CallStack::new();

        assert_eq!(calls.depth(), 1);
        assert_eq!(calls.argument_count(), 0);

        let error = calls.pop_frame().unwrap_err();
        assert!(error.to_string().contains("§6.4"));
    }

    #[test]
    fn arguments_overwrite_the_first_locals() {
        let mut calls = CallStack::new();
        calls
            .call(&routine(&[0x11, 0x22, 0x33]), &[0xAA], 0x500, Some(0))
            .unwrap();

        assert_eq!(calls.local(1).unwrap(), 0xAA);
        assert_eq!(calls.local(2).unwrap(), 0x22);
        assert_eq!(calls.local(3).unwrap(), 0x33);
        assert_eq!(calls.argument_count(), 1);
    }

    #[test]
    fn spare_arguments_are_thrown_away() {
        let mut calls = CallStack::new();
        calls
            .call(&routine(&[0x11]), &[1, 2, 3], 0x500, None)
            .unwrap();

        assert_eq!(calls.local(1).unwrap(), 1);
        assert_eq!(calls.argument_count(), 3);
        assert!(calls.local(2).is_err());
    }

    #[test]
    fn locals_that_do_not_exist_cannot_be_touched() {
        let mut calls = CallStack::new();

        assert!(calls.local(1).is_err());
        assert!(calls.set_local(1, 5).is_err());

        calls.call(&routine(&[0]), &[], 0, None).unwrap();

        assert!(calls.local(0).is_err());
        assert!(calls.local(2).is_err());
    }

    #[test]
    fn locals_can_be_written() {
        let mut calls = CallStack::new();
        calls.call(&routine(&[0, 0]), &[], 0, None).unwrap();

        calls.set_local(2, 0xBEEF).unwrap();

        assert_eq!(calls.local(2).unwrap(), 0xBEEF);
    }

    #[test]
    fn each_routine_sees_only_its_own_stack() {
        let mut calls = CallStack::new();
        calls.push(0x1111).unwrap();

        calls.call(&routine(&[]), &[], 0, None).unwrap();

        assert!(calls.pop().is_err());

        calls.push(0x2222).unwrap();
        assert_eq!(calls.pop().unwrap(), 0x2222);

        calls.pop_frame().unwrap();
        assert_eq!(calls.pop().unwrap(), 0x1111);
    }

    #[test]
    fn runaway_recursion_hits_the_usage_ceiling() {
        let mut calls = CallStack::new();
        let recursing = routine(&[0; 15]);

        let error = loop {
            if let Err(error) = calls.call(&recursing, &[], 0, None) {
                break error;
            }
        };

        assert!(error.to_string().contains("runaway recursion"));
    }

    #[test]
    fn runaway_pushing_hits_the_ceiling_too() {
        let mut calls = CallStack::new();

        let error = loop {
            if let Err(error) = calls.push(1) {
                break error;
            }
        };

        assert!(error.to_string().contains("§6.3.3"));
    }

    #[test]
    fn balanced_calls_reclaim_their_usage() {
        let mut calls = CallStack::new();
        let heavy = routine(&[0; 15]);

        for _ in 0..100_000 {
            calls.call(&heavy, &[], 0, None).unwrap();
            calls.pop_frame().unwrap();
        }

        assert_eq!(calls.depth(), 1);
    }

    #[test]
    fn in_place_access_needs_a_stack_top() {
        let mut calls = CallStack::new();

        assert!(calls.peek().is_err());
        assert!(calls.replace_top(1).is_err());
    }

    #[test]
    fn replace_top_overwrites_without_growing() {
        let mut calls = CallStack::new();
        calls.push(1).unwrap();

        calls.replace_top(2).unwrap();

        assert_eq!(calls.peek().unwrap(), 2);
        assert_eq!(calls.pop().unwrap(), 2);
        assert!(calls.pop().is_err());
    }

    #[test]
    fn popped_frames_carry_their_return_directions() {
        let mut calls = CallStack::new();
        calls.call(&routine(&[]), &[], 0x1234, Some(0x10)).unwrap();

        let direction = calls.pop_frame().unwrap();

        assert_eq!(direction.return_address, 0x1234);
        assert_eq!(direction.store_variable, Some(0x10));
    }

    #[test]
    fn call_chain_survives_a_snapshot_round_trip() {
        let mut calls = CallStack::new();
        calls.push(7).unwrap();
        calls.call(&routine(&[1, 2]), &[9], 0x500, Some(3)).unwrap();
        calls.push(0xAB).unwrap();

        let capture = calls.snapshot();

        calls.pop_frame().unwrap();
        calls.restore(&capture).unwrap();

        assert_eq!(calls.depth(), 2);
        assert_eq!(calls.local(1).unwrap(), 9);
        assert_eq!(calls.pop().unwrap(), 0xAB);

        let direction = calls.pop_frame().unwrap();
        assert_eq!(direction.return_address, 0x500);
        assert_eq!(calls.pop().unwrap(), 7);
    }

    #[test]
    fn restore_recomputes_usage_and_the_chain_still_pops() {
        let mut calls = CallStack::new();
        calls.call(&routine(&[0; 5]), &[], 0, None).unwrap();

        let capture = calls.snapshot();
        let mut fresh = CallStack::new();
        fresh.restore(&capture).unwrap();

        assert_eq!(fresh.depth(), 2);
        fresh.pop_frame().unwrap();
        assert_eq!(fresh.depth(), 1);
    }

    #[test]
    fn restoring_an_empty_call_chain_is_refused() {
        let mut calls = CallStack::new();

        assert!(calls.restore(&[]).is_err());
    }

    #[test]
    fn restoring_an_impossible_call_chain_is_refused() {
        let mut calls = CallStack::new();
        let bloated = vec![
            FrameSnapshot {
                return_address: 0,
                store_variable: None,
                locals: Vec::new(),
                argument_count: 0,
                stack: vec![0; USAGE_LIMIT + 1],
            };
            1
        ];

        let error = calls.restore(&bloated).unwrap_err();
        assert!(error.to_string().contains("honest capture"));
    }
}
