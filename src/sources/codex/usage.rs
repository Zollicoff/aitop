//! Codex's token counters and the arithmetic the billing rules need.

use serde_json::Value;

/// Token counters as Codex reports them: `input` includes the cached and
/// cache-write parts, `output` includes reasoning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Usage {
    pub(super) input: u64,
    pub(super) cached: u64,
    pub(super) cache_write: u64,
    pub(super) output: u64,
    pub(super) reasoning: u64,
    pub(super) total: u64,
}

impl Usage {
    pub(super) fn read(v: Option<&Value>) -> Option<Usage> {
        let o = v?.as_object()?;
        // A counter that is present must be a whole number.
        let get = |k: &str, default: u64| match o.get(k) {
            None => Some(default),
            Some(x) => x.as_u64(),
        };
        let input = o.get("input_tokens")?.as_u64()?;
        let output = o.get("output_tokens")?.as_u64()?;
        let u = Usage {
            input,
            cached: get("cached_input_tokens", 0)?,
            cache_write: get("cache_write_input_tokens", 0)?,
            output,
            reasoning: get("reasoning_output_tokens", 0)?,
            total: get("total_tokens", input + output)?,
        };
        u.valid().then_some(u)
    }

    fn valid(&self) -> bool {
        self.cached + self.cache_write <= self.input
            && self.reasoning <= self.output
            && self.total >= self.input + self.output
    }

    pub(super) fn fields(&self) -> [u64; 6] {
        [self.input, self.cached, self.cache_write, self.output, self.reasoning, self.total]
    }

    fn from_fields(f: [u64; 6]) -> Option<Usage> {
        let u = Usage { input: f[0], cached: f[1], cache_write: f[2], output: f[3], reasoning: f[4], total: f[5] };
        u.valid().then_some(u)
    }

    pub(super) fn plus(&self, o: &Usage) -> Option<Usage> {
        let (a, b) = (self.fields(), o.fields());
        Usage::from_fields(std::array::from_fn(|i| a[i] + b[i]))
    }

    /// `self - o`, if no counter goes below zero and what is left is usage.
    pub(super) fn minus(&self, o: &Usage) -> Option<Usage> {
        let (a, b) = (self.fields(), o.fields());
        let mut out = [0; 6];
        for i in 0..6 {
            out[i] = a[i].checked_sub(b[i])?;
        }
        Usage::from_fields(out)
    }

    /// Every counter of `self` is exactly `o` more than `prev`'s.
    pub(super) fn is_step_from(&self, prev: &Usage, o: &Usage) -> bool {
        let (a, p, b) = (self.fields(), prev.fields(), o.fields());
        (0..6).all(|i| a[i].checked_sub(p[i]) == Some(b[i]))
    }

    pub(super) fn below(&self, o: &Usage) -> bool {
        self.fields().iter().zip(o.fields()).any(|(a, b)| *a < b)
    }

    /// Nothing was sent or generated: a context-size marker, not a request.
    pub(super) fn is_empty(&self) -> bool {
        self.input + self.output == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counters() {
        let read = |j: &str| Usage::read(Some(&serde_json::from_str::<Value>(j).unwrap()));
        let u = read(r#"{"input_tokens":10,"cached_input_tokens":4,"output_tokens":5}"#).unwrap();
        assert_eq!((u.total, u.cached, u.reasoning), (15, 4, 0));
        assert!(
            read(r#"{"input_tokens":10,"cached_input_tokens":40,"output_tokens":5}"#).is_none(),
            "more cached than sent"
        );
        assert!(read(r#"{"input_tokens":10,"output_tokens":5,"total_tokens":3}"#).is_none());
        assert!(read(r#"{"input_tokens":10.5,"output_tokens":5}"#).is_none());
        assert!(read(r#"{"output_tokens":5}"#).is_none());
        let twice = u.plus(&u).unwrap();
        assert_eq!(twice.minus(&u), Some(u));
        assert_eq!(u.minus(&twice), None);
        assert!(twice.is_step_from(&u, &u) && !twice.is_step_from(&u, &twice));
    }
}
