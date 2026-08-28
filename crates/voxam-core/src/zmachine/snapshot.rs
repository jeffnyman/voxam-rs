//! A captured state of play (§6.1).
//!
//! The state of play is four things: the contents of dynamic
//! memory, the contents of the stack, the program counter, and the
//! routine call state -- the chain of routines that have called
//! each other, with their local variables (§6.1). A Snapshot holds
//! all four as owned values in the interpreter's private memory,
//! exactly where §6.1 says the stack and call state must live.
//!
//! A Snapshot is the common currency of every state-travel feature:
//! save writes one out, restore plays one back, undo keeps one in
//! hand, and Quetzal will one day be a Snapshot in a file.

/// One frozen link of the routine call chain (§6.1): what the call
/// state remembers about one routine invocation, without the
/// ability to mutate a live machine through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSnapshot {
    /// Where execution resumes when the routine returns (§6.4).
    pub return_address: usize,
    /// The caller's variable for the result, or `None` when the
    /// result is thrown away (§6.4.1).
    pub store_variable: Option<u8>,
    /// The routine's local variable values, in order.
    pub locals: Vec<u16>,
    /// How many arguments the caller supplied (§6.4.4.1).
    pub argument_count: usize,
    /// The routine's private portion of the stack (§6.3.2).
    pub stack: Vec<u16>,
}

/// The entire state of play, captured whole (§6.1, §6.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Every byte below the static memory base, header included
    /// (§1.1.1).
    pub dynamic_memory: Vec<u8>,
    /// The byte address of the next instruction to execute.
    pub pc: usize,
    /// The routine call chain from the base frame up, each with its
    /// locals and its portion of the stack (§6.1).
    pub frames: Vec<FrameSnapshot>,
}
