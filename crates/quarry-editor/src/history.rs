//! Undo history. Each step is a group of transactions (one Helix insert session, one Normal
//! command, or one run of Standard typing) with the selections to restore on either side.

use crate::selection::Selections;
use crate::transaction::Transaction;

#[derive(Clone, Debug)]
pub struct Step {
    pub id: u64,
    /// Forward transactions in application order.
    pub forward: Vec<Transaction>,
    /// Inverse transactions, same order as `forward` (undo applies them in reverse).
    pub inverse: Vec<Transaction>,
    pub before: Selections,
    pub after: Selections,
}

#[derive(Default)]
pub struct History {
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    next_id: u64,
}

impl History {
    pub fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Id of the state the document is in (0 = pristine).
    pub fn current_id(&self) -> u64 {
        self.undo.last().map_or(0, |s| s.id)
    }
}
