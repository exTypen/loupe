//! Stash apply, pop and drop (SS-06..10). Swift drew them in History's Stashes scope; the History view owns that
//! list here too, and calls these. Each records its undo (core's `Recipe::Stash`), and an apply or pop that stops
//! on conflicts is not an error: the caller switches to Changes, where the banner and the merge editor take over.
use gpui_kit::{Context, Task};
use loupe_core::git;
use loupe_core::undo::{Recipe, UndoStack};

use crate::state::WorktreeModel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StashOp { Apply, Pop, Drop }

impl StashOp {
    fn label(self) -> &'static str { match self { StashOp::Apply => "Apply Stash", StashOp::Pop => "Pop Stash", StashOp::Drop => "Drop Stash" } }
    fn verb(self) -> &'static str { match self { StashOp::Apply => "apply", StashOp::Pop => "pop", StashOp::Drop => "drop" } }
}

/// SS-08's confirmation, for the caller's sheet.
pub fn drop_question(selector: &str) -> (String, &'static str) {
    (format!("Drop {selector}?"), "The stash is removed from the list. Edit › Undo brings it back.")
}

impl WorktreeModel {
    /// `git stash apply|pop|drop <selector>` (stash@{n}) with its undo. `sha` and `subject` are the stash commit's,
    /// so Undo can `git stash store` it again. The task gives `Some(true)` when it stopped on conflicts (SS-10).
    pub fn stash_op(&mut self, op: StashOp, selector: String, sha: String, subject: String, cx: &mut Context<Self>) -> Task<Option<bool>> {
        let busy = match op { StashOp::Apply => "Applying stash", StashOp::Pop => "Popping stash", StashOp::Drop => "Dropping stash" };
        self.perform(busy, move |p| {
            let recipe = Recipe::Stash { label: op.label().into(), sha, message: subject, applied: op != StashOp::Drop, dropped: op != StashOp::Apply };
            let r = UndoStack::for_path(p).run(Some(recipe), || git::run(p, ["stash", op.verb(), selector.as_str()]).map(drop));
            match r {
                Ok(()) => Ok(false),
                // A conflict is where the user takes over, not a failure (git keeps the stash on a pop).
                Err(e) if op != StashOp::Drop && conflicted(p) => { let _ = e; Ok(true) }
                Err(e) => Err(e),
            }
        }, cx)
    }
}

fn conflicted(p: &std::path::Path) -> bool {
    git::run(p, ["diff", "--name-only", "--diff-filter=U"]).is_ok_and(|o| !o.iter().all(u8::is_ascii_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_question() {
        assert_eq!(StashOp::Pop.label(), "Pop Stash");
        assert_eq!(drop_question("stash@{1}").0, "Drop stash@{1}?");
    }
}
