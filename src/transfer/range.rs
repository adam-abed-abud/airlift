//! Parsing of the HTTP `Range` request header (RFC 9110 §14).
//!
//! Only single byte ranges are honoured. Multipart ranges are legal but no
//! browser media player uses them, and answering with the whole file is a
//! valid response to any range request, so that is what happens for them.

/// Inclusive byte range within a file of known size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RangeOutcome {
    /// Serve the whole file (no header, unsupported form, or multiple ranges).
    Full,
    /// Serve this slice with 206 Partial Content.
    Partial(ByteRange),
    /// Header was syntactically fine but asks for bytes that do not exist:
    /// answer 416 Range Not Satisfiable.
    Unsatisfiable,
}

pub fn parse(header: Option<&str>, size: u64) -> RangeOutcome {
    let Some(header) = header else {
        return RangeOutcome::Full;
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeOutcome::Full;
    };
    if spec.contains(',') {
        return RangeOutcome::Full;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeOutcome::Full;
    };
    let (start, end) = (start.trim(), end.trim());

    if size == 0 {
        // Any range on an empty body is unsatisfiable by definition.
        return RangeOutcome::Unsatisfiable;
    }

    match (start.is_empty(), end.is_empty()) {
        // `bytes=-500`: the final 500 bytes.
        (true, false) => match end.parse::<u64>() {
            Ok(0) => RangeOutcome::Unsatisfiable,
            Ok(n) => RangeOutcome::Partial(ByteRange { start: size.saturating_sub(n), end: size - 1 }),
            Err(_) => RangeOutcome::Full,
        },
        // `bytes=500-`: from 500 to the end.
        (false, true) => match start.parse::<u64>() {
            Ok(s) if s < size => RangeOutcome::Partial(ByteRange { start: s, end: size - 1 }),
            Ok(_) => RangeOutcome::Unsatisfiable,
            Err(_) => RangeOutcome::Full,
        },
        // `bytes=500-999`.
        (false, false) => match (start.parse::<u64>(), end.parse::<u64>()) {
            (Ok(s), Ok(e)) if s <= e && s < size => {
                RangeOutcome::Partial(ByteRange { start: s, end: e.min(size - 1) })
            }
            (Ok(_), Ok(_)) => RangeOutcome::Unsatisfiable,
            _ => RangeOutcome::Full,
        },
        (true, true) => RangeOutcome::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(start: u64, end: u64) -> RangeOutcome {
        RangeOutcome::Partial(ByteRange { start, end })
    }

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse(None, 100), RangeOutcome::Full);
        assert_eq!(parse(Some("bytes=0-"), 100), part(0, 99));
        assert_eq!(parse(Some("bytes=10-19"), 100), part(10, 19));
        assert_eq!(parse(Some("bytes=90-200"), 100), part(90, 99));
        assert_eq!(parse(Some("bytes=-10"), 100), part(90, 99));
        assert_eq!(parse(Some("bytes=-1000"), 100), part(0, 99));
    }

    #[test]
    fn rejects_or_ignores_bad_forms() {
        assert_eq!(parse(Some("bytes=100-"), 100), RangeOutcome::Unsatisfiable);
        assert_eq!(parse(Some("bytes=50-40"), 100), RangeOutcome::Unsatisfiable);
        assert_eq!(parse(Some("bytes=0-"), 0), RangeOutcome::Unsatisfiable);
        assert_eq!(parse(Some("bytes=0-10,20-30"), 100), RangeOutcome::Full);
        assert_eq!(parse(Some("items=0-10"), 100), RangeOutcome::Full);
        assert_eq!(parse(Some("bytes=abc-"), 100), RangeOutcome::Full);
    }
}
