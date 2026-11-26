use std::ops::Range;

/// Assemble a byte string from ordered edits to the original input.
pub struct ByteEdits<'a> {
    input: &'a [u8],
    output: Vec<u8>,
    copied_until: Option<usize>,
}

impl<'a> ByteEdits<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            output: Vec::new(),
            copied_until: None,
        }
    }

    /// Replace a range in the original input. An empty range inserts bytes.
    /// Panics if the range is out of bounds, reversed, or overlaps an earlier edit.
    pub fn replace(&mut self, range: impl Into<Range<usize>>, replacement: &[u8]) {
        let range = range.into();
        let start = self.copied_until.unwrap_or(0);
        assert!(start <= range.start && range.start <= range.end && range.end <= self.input.len());
        self.output
            .extend_from_slice(&self.input[start..range.start]);
        self.output.extend_from_slice(replacement);
        self.copied_until = Some(range.end);
    }

    /// Return the edited bytes, or `None` if no edits were made.
    pub fn finish(mut self) -> Option<Vec<u8>> {
        self.output
            .extend_from_slice(&self.input[self.copied_until?..]);
        Some(self.output)
    }
}
