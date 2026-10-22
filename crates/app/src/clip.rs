//! Clipboard state shared by all windows: the macOS pasteboard plus the
//! last block we copied, so pastes within Rustsheet keep formulas.

use rustsheet_core::Range;
use rustsheet_core::clipboard::Block;

/// Where a pending cut came from; cleared once the cut is pasted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutSource {
    pub window: u64,
    pub range: Range,
}

pub struct ClipState {
    board: Option<arboard::Clipboard>,
    /// Stands in for the pasteboard when it is unavailable (and in tests).
    local: Option<String>,
    block: Option<Block>,
    /// The text we put on the pasteboard with `block`.
    text: String,
    cut: Option<CutSource>,
}

/// What a paste should insert.
pub enum Pasted {
    /// Our own cells, with the cut source if it was a cut.
    Internal(Block, Option<CutSource>),
    /// Text from another application.
    External(Block),
}

impl ClipState {
    pub fn new() -> Self {
        Self {
            board: arboard::Clipboard::new().ok(),
            local: None,
            block: None,
            text: String::new(),
            cut: None,
        }
    }

    /// A clipboard that never touches the system pasteboard.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self { board: None, local: None, block: None, text: String::new(), cut: None }
    }

    /// Puts plain text on the clipboard, as another application would.
    #[cfg(test)]
    pub fn set_foreign_text(&mut self, text: &str) {
        self.local = Some(text.to_string());
    }

    pub fn set(&mut self, block: Block, text: String, cut: Option<CutSource>) {
        match &mut self.board {
            Some(board) => {
                let _ = board.set_text(text.clone());
            }
            None => self.local = Some(text.clone()),
        }
        self.block = Some(block);
        self.text = text;
        self.cut = cut;
    }

    /// Plain text on the pasteboard, for pasting into a text field.
    pub fn text(&mut self) -> Option<String> {
        match &mut self.board {
            Some(board) => board.get_text().ok(),
            None => self.local.clone(),
        }
    }

    /// Decides what a paste inserts. A cut can be pasted only once, like in Excel.
    pub fn take_paste(&mut self) -> Option<Pasted> {
        let text = self.text();
        let ours = match (&text, &self.block) {
            (Some(t), Some(_)) => *t == self.text,
            (None, Some(_)) => true,
            _ => false,
        };
        if ours {
            let block = self.block.clone()?;
            let cut = self.cut.take();
            if cut.is_some() {
                self.block = None;
            }
            return Some(Pasted::Internal(block, cut));
        }
        text.filter(|t| !t.is_empty()).map(|t| Pasted::External(Block::from_tsv(&t)))
    }

    /// The pending cut, so the view can draw its marching border.
    pub fn cut(&self) -> Option<CutSource> {
        self.cut
    }

    pub fn cancel_cut(&mut self) {
        self.cut = None;
    }
}
