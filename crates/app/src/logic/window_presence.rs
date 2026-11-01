#[derive(Default)]
pub(crate) struct WindowPresence {
    hidden: bool,
    ghosted: bool,
    quiet: bool,
}

impl WindowPresence {
    pub(crate) fn is_hidden(&self) -> bool {
        self.hidden
    }

    pub(crate) fn is_ghosted(&self) -> bool {
        self.ghosted
    }

    pub(crate) fn is_quiet(&self) -> bool {
        self.quiet
    }

    pub(crate) fn set_quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    pub(crate) fn set_ghosted(&mut self, ghosted: bool) {
        self.ghosted = ghosted;
    }

    pub(crate) fn take_ghost(&mut self) -> bool {
        std::mem::take(&mut self.ghosted)
    }

    pub(crate) fn hide(&mut self) -> bool {
        if self.hidden {
            return false;
        }
        self.hidden = true;
        true
    }

    pub(crate) fn show(&mut self) -> bool {
        self.hidden = false;
        self.take_ghost()
    }

    #[cfg(test)]
    pub(crate) fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }
}
