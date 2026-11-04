#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
    pub unsupported: usize,
}

impl Summary {
    pub fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Passed => self.passed += 1,
            Outcome::Failed(_) => self.failed += 1,
            Outcome::Unsupported(_) => self.unsupported += 1,
        }
    }

    pub fn merge(&mut self, other: Summary) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.unsupported += other.unsupported;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed(String),
    Unsupported(String),
}
