#[derive(Default)]
pub(crate) struct WindowPresence {
    ghosted: bool,
    quiet: bool,
}

impl WindowPresence {
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
}
